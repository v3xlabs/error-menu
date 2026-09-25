//! Where GitHub tells the app about its installs and about new commits. A webhook is a
//! nudge into the queue polling already feeds, never a second way to scan: a lost delivery
//! costs latency, and the next poll finds the change anyway.

use std::sync::Arc;

use poem::http::StatusCode;
use poem::web::Data;
use poem::{Body, Request, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::app::AppState;
use crate::forge::github::app::GithubApp;
use crate::forge::github::installation::{CoveredRepository, GithubInstallation};
use crate::forge::report::{Reporting, Step};
use crate::prelude::*;
use crate::worker::discovery;
use crate::worker::queue::{Job, JobKind};

/// GitHub caps a delivery at 25 MB.
const MAX_DELIVERY_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
enum WebhookError {
    #[error("the delivery is not the payload its event names: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("the delivery names an invalid commit")]
    Commit,
    #[error("{0}")]
    Database(#[from] DatabaseError),
}

#[derive(Deserialize)]
struct InstallationEvent {
    action: String,
    installation: InstallationRef,
    #[serde(default)]
    repositories: Vec<RepositoryRef>,
}

#[derive(Deserialize)]
struct InstallationRepositoriesEvent {
    installation: InstallationRef,
    #[serde(default)]
    repositories_added: Vec<RepositoryRef>,
    #[serde(default)]
    repositories_removed: Vec<RepositoryRef>,
}

#[derive(Deserialize)]
struct PullRequestEvent {
    action: String,
    installation: Option<InstallationRef>,
    repository: RepositoryRef,
    pull_request: PullRequestRef,
}

#[derive(Deserialize)]
struct PullRequestRef {
    head: HeadRef,
}

#[derive(Deserialize)]
struct HeadRef {
    sha: String,
}

#[derive(Deserialize)]
struct PushEvent {
    #[serde(rename = "ref")]
    reference: String,
    after: String,
    #[serde(default)]
    deleted: bool,
    installation: Option<InstallationRef>,
    repository: PushRepository,
}

#[derive(Deserialize)]
struct PushRepository {
    id: i64,
    full_name: String,
    default_branch: String,
}

/// `check_run` and `check_suite` share every field a re-run needs, under their own key.
#[derive(Deserialize)]
struct RerunEvent {
    action: String,
    #[serde(alias = "check_suite")]
    check_run: RerunTarget,
    repository: RepositoryRef,
    sender: Sender,
}

/// The GitHub account whose action caused a delivery. For a re-run it is who pressed the
/// button, which is the one thing that tells an honest re-run from someone keeping the
/// server busy.
#[derive(Deserialize)]
struct Sender {
    login: String,
    id: i64,
}

#[derive(Deserialize)]
struct RerunTarget {
    head_sha: String,
    app: Option<AppRef>,
}

#[derive(Deserialize)]
struct AppRef {
    id: u64,
}

#[derive(Deserialize)]
struct InstallationRef {
    id: i64,
}

#[derive(Deserialize)]
struct RepositoryRef {
    id: i64,
    full_name: String,
}

impl From<RepositoryRef> for CoveredRepository {
    fn from(repository: RepositoryRef) -> Self {
        CoveredRepository {
            id: repository.id,
            full_name: repository.full_name,
        }
    }
}

#[poem::handler]
pub async fn github(Data(state): Data<&Arc<AppState>>, request: &Request, body: Body) -> Response {
    let Some(app) = state.github_app.as_deref() else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(body) = body.into_bytes_limit(MAX_DELIVERY_BYTES).await else {
        return status(StatusCode::PAYLOAD_TOO_LARGE);
    };
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    if !header("x-hub-signature-256").is_some_and(|signature| app.signed(&body, signature)) {
        return status(StatusCode::UNAUTHORIZED);
    }
    let event = header("x-github-event").unwrap_or_default();

    match handle(state, app, event, &body).await {
        Ok(()) => status(StatusCode::NO_CONTENT),
        Err(error @ (WebhookError::Payload(_) | WebhookError::Commit)) => {
            tracing::warn!(%event, %error, "refused a GitHub delivery");
            status(StatusCode::BAD_REQUEST)
        }
        // GitHub shows a failed delivery and can send it again, so the answer says so.
        Err(error @ WebhookError::Database(_)) => {
            tracing::error!(%event, %error, "could not act on a GitHub delivery");
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn handle(
    state: &Arc<AppState>,
    app: &GithubApp,
    event: &str,
    body: &[u8],
) -> Result<(), WebhookError> {
    match event {
        "installation" => {
            let event: InstallationEvent = payload(body)?;
            let installation = GithubInstallation {
                id: event.installation.id,
            };
            match event.action.as_str() {
                "created" | "unsuspend" | "new_permissions_accepted" => {
                    let covered = into_covered(event.repositories);
                    installation.cover(&state.database, &covered).await?;
                }
                "deleted" | "suspend" => installation.remove(&state.database).await?,
                _ => {}
            }
        }
        "installation_repositories" => {
            let event: InstallationRepositoriesEvent = payload(body)?;
            let installation = GithubInstallation {
                id: event.installation.id,
            };
            installation
                .cover(&state.database, &into_covered(event.repositories_added))
                .await?;
            let removed = event
                .repositories_removed
                .iter()
                .map(|repository| repository.id)
                .collect::<Vec<_>>();
            installation.uncover(&state.database, &removed).await?;
        }
        "pull_request" => {
            let event: PullRequestEvent = payload(body)?;
            if matches!(
                event.action.as_str(),
                "opened" | "synchronize" | "reopened" | "ready_for_review"
            ) {
                let head = commit(&event.pull_request.head.sha)?;
                cover(state, event.installation, &event.repository).await?;
                moved(state, &event.repository.full_name, head).await?;
            }
        }
        "push" => {
            let event: PushEvent = payload(body)?;
            let default = format!("refs/heads/{}", event.repository.default_branch);
            if event.reference == default && !event.deleted {
                let head = commit(&event.after)?;
                let repository = RepositoryRef {
                    id: event.repository.id,
                    full_name: event.repository.full_name,
                };
                cover(state, event.installation, &repository).await?;
                moved(state, &repository.full_name, head).await?;
            }
        }
        "check_run" | "check_suite" => {
            let event: RerunEvent = payload(body)?;
            let ours = event.check_run.app.map(|app| app.id) == Some(app.id());
            if event.action == "rerequested" && ours {
                let head = commit(&event.check_run.head_sha)?;
                tracing::info!(
                    sender = %event.sender.login,
                    sender_id = event.sender.id,
                    repository = %event.repository.full_name,
                    %head,
                    "a re-run was asked for on GitHub"
                );
                rerun(state, &event.repository.full_name, head).await?;
            }
        }
        _ => {}
    }

    Ok(())
}

/// A webhook names the installation it came through, so a repository that was renamed,
/// or that joined the installation while error.menu was down, is covered again.
async fn cover(
    state: &AppState,
    installation: Option<InstallationRef>,
    repository: &RepositoryRef,
) -> Result<(), DatabaseError> {
    let Some(installation) = installation else {
        return Ok(());
    };
    GithubInstallation {
        id: installation.id,
    }
    .cover(
        &state.database,
        &[CoveredRepository {
            id: repository.id,
            full_name: repository.full_name.clone(),
        }],
    )
    .await
}

/// A head moved on a watched repository. Discovery reads it on its next claim, and the
/// check shows as queued on the forge before that claim comes.
async fn moved(
    state: &Arc<AppState>,
    full_name: &str,
    head: CommitSha,
) -> Result<(), DatabaseError> {
    for project in Project::on_github(&state.database, full_name).await? {
        Job::enqueue(&state.database, project.id, JobKind::Discover).await?;
        let state = Arc::clone(state);
        let head = head.clone();
        tokio::spawn(async move {
            match Reporting::for_project(&state, &project).await {
                Ok(Some(reporting)) => {
                    reporting.record(&state.database, &head, Step::Queued).await;
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "could not queue the forge check"),
            }
        });
    }
    state.queue_wake.notify_one();

    Ok(())
}

/// Someone pressed Re-run on the forge. A pull request head is scanned again; any other
/// head has nothing new to read, so its check is dropped and the next pass writes a fresh
/// one from the stored analysis.
async fn rerun(
    state: &Arc<AppState>,
    full_name: &str,
    head: CommitSha,
) -> Result<(), DatabaseError> {
    for project in Project::on_github(&state.database, full_name).await? {
        match Snapshot::change_with_head(&state.database, project.id, &head).await? {
            Some(number) => {
                let state = Arc::clone(state);
                tokio::spawn(async move {
                    if let Err(error) = discovery::scan_change(&state, project.id, number).await {
                        tracing::warn!(%error, number, "the re-run asked for on GitHub failed");
                    }
                });
            }
            None => {
                if let Some(reporting) = Reporting::for_project(state, &project).await? {
                    reporting.forget(&state.database, &head).await?;
                }
                Job::enqueue(&state.database, project.id, JobKind::Discover).await?;
            }
        }
    }
    state.queue_wake.notify_one();

    Ok(())
}

fn payload<Value: DeserializeOwned>(body: &[u8]) -> Result<Value, WebhookError> {
    Ok(serde_json::from_slice(body)?)
}

fn commit(sha: &str) -> Result<CommitSha, WebhookError> {
    CommitSha::new(sha).map_err(|_| WebhookError::Commit)
}

fn into_covered(repositories: Vec<RepositoryRef>) -> Vec<CoveredRepository> {
    repositories.into_iter().map(Into::into).collect()
}

fn status(code: StatusCode) -> Response {
    Response::builder().status(code).finish()
}
