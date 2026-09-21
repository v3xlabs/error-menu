//! GitLab. It nests projects under groups, numbers changes per project, and keeps CI in
//! pipelines of jobs rather than checks on a commit, so almost nothing here is shared.

use serde::Deserialize;

use super::reader::{Api, ApiBase, ForgeReadError, PAGE_SIZE, optional_sha, sha};
use super::{
    ChangeState, CommitReading, DiscoveredChange, Forge, ForgeMetadata, check_conclusion,
    check_status,
};
use crate::analysis::ci_checks::CheckRun;
use crate::prelude::*;

pub(crate) struct Gitlab;

impl Forge for Gitlab {
    fn api_base(_host: &str, authority: &str) -> Result<ApiBase, ForgeReadError> {
        ApiBase::new(&format!("https://{authority}/"), &["api", "v4"])
    }

    fn change_ref(number: u64) -> String {
        format!("refs/merge-requests/{number}/head")
    }

    /// GitLab names a project by its id or by its path encoded as one segment, so the
    /// numeric id it would cost a request to learn is never needed here.
    async fn changes(api: &Api<'_>) -> Result<Vec<DiscoveredChange>, ForgeReadError> {
        let changes: Vec<GitlabChange> = api
            .get_with_query(
                api.repository.endpoint(&[
                    "projects",
                    &api.repository.project_path(),
                    "merge_requests",
                ]),
                &[
                    ("scope", "all"),
                    ("order_by", "updated_at"),
                    ("sort", "desc"),
                    ("per_page", PAGE_SIZE),
                ],
            )
            .await?;

        changes.into_iter().map(TryInto::try_into).collect()
    }

    /// GitLab's signature endpoint carries no accounts, so the join between the address a
    /// commit was written with and a forge login is one this forge cannot answer.
    async fn read_commit(api: &Api<'_>, head: &CommitSha) -> Result<CommitReading, ForgeReadError> {
        let endpoint = api.repository.endpoint(&[
            "projects",
            &api.repository.project_path(),
            "repository",
            "commits",
            head.as_str(),
            "signature",
        ]);

        match api.get::<GitlabSignature>(endpoint).await {
            Ok(signature) => Ok(CommitReading {
                signature: Signature {
                    present: true,
                    verified: Some(signature.verification_status == "verified"),
                    signer: signature.gpg_key_user_name,
                    reason: Some(signature.verification_status),
                },
                accounts: Vec::new(),
            }),
            // GitLab answers 404 when the commit carries no signature at all.
            Err(ForgeReadError::Request(_)) => Ok(CommitReading::default()),
            Err(error) => Err(error),
        }
    }

    async fn check_runs(api: &Api<'_>, head: &CommitSha) -> Result<Vec<CheckRun>, ForgeReadError> {
        let project: GitlabProject = api
            .get(
                api.repository
                    .endpoint(&["projects", &api.repository.project_path()]),
            )
            .await?;
        let project_id = project.id.to_string();
        let pipelines: Vec<GitlabPipeline> = api
            .get_with_query(
                api.repository
                    .endpoint(&["projects", &project_id, "pipelines"]),
                &[("sha", head.as_str()), ("per_page", PAGE_SIZE)],
            )
            .await?;
        let mut check_runs = Vec::new();

        for pipeline in pipelines {
            let pipeline_id = pipeline.id.to_string();
            let jobs: Vec<GitlabJob> = api
                .get_with_query(
                    api.repository.endpoint(&[
                        "projects",
                        &project_id,
                        "pipelines",
                        &pipeline_id,
                        "jobs",
                    ]),
                    &[("per_page", PAGE_SIZE)],
                )
                .await?;
            check_runs.extend(
                jobs.into_iter()
                    .map(|job| job.into_check_run(api, &project_id)),
            );
        }

        Ok(check_runs)
    }
}

#[derive(Deserialize)]
struct GitlabPipeline {
    id: u64,
}

#[derive(Deserialize)]
struct GitlabJob {
    id: u64,
    name: String,
    status: String,
    #[serde(default)]
    web_url: Option<String>,
}

impl GitlabJob {
    fn into_check_run(self, api: &Api<'_>, project_id: &str) -> CheckRun {
        let job_id = self.id.to_string();
        let log_excerpt_ref = api
            .repository
            .endpoint(&["projects", project_id, "jobs", &job_id, "trace"])
            .to_string();

        CheckRun {
            name: self.name,
            status: check_status(&self.status),
            conclusion: check_conclusion(&self.status),
            url: self.web_url,
            log_excerpt_ref: Some(log_excerpt_ref),
        }
    }
}

#[derive(Deserialize)]
struct GitlabProject {
    id: u64,
}

#[derive(Deserialize)]
struct GitlabChange {
    iid: u64,
    web_url: String,
    title: String,
    description: Option<String>,
    state: String,
    merge_commit_sha: Option<String>,
    author: GitlabUser,
    #[serde(default)]
    reviewers: Vec<GitlabUser>,
    target_branch: String,
    source_branch: String,
    sha: String,
    diff_refs: GitlabDiffReferences,
}

#[derive(Deserialize)]
struct GitlabUser {
    username: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GitlabDiffReferences {
    base_sha: String,
}

impl GitlabUser {
    fn person(self, role: PersonRole) -> Person {
        Person {
            role,
            name: self.name,
            email: None,
            login: Some(self.username),
            avatar_url: self.avatar_url,
        }
    }
}

impl TryFrom<GitlabChange> for DiscoveredChange {
    type Error = ForgeReadError;

    fn try_from(change: GitlabChange) -> Result<Self, Self::Error> {
        let state = match change.state.as_str() {
            "merged" => ChangeState::Merged,
            "closed" | "locked" => ChangeState::Closed,
            _ => ChangeState::Open,
        };
        let mut people = vec![change.author.person(PersonRole::Submitter)];
        people.extend(
            change
                .reviewers
                .into_iter()
                .map(|user| user.person(PersonRole::Reviewer)),
        );

        Ok(Self {
            number: change.iid,
            fetch_ref: Gitlab::change_ref(change.iid),
            base: sha("merge request base", &change.diff_refs.base_sha)?,
            head: sha("merge request head", &change.sha)?,
            metadata: ForgeMetadata {
                title: Some(change.title),
                body: change.description,
                author: people.first().and_then(|person| person.login.clone()),
                url: Some(change.web_url),
                base_ref: Some(change.target_branch),
                head_ref: Some(change.source_branch),
                state: Some(state),
                merge_commit: optional_sha(change.merge_commit_sha.as_deref()),
                people: crate::project::person::deduplicate(people),
                signature: Signature::default(),
            },
        })
    }
}

#[derive(Deserialize)]
struct GitlabSignature {
    verification_status: String,
    gpg_key_user_name: Option<String>,
}
