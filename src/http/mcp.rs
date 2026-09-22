use std::sync::Arc;

use poem::http::{HeaderValue, header};
use poem::{EndpointExt, IntoEndpoint, Request};
use poem_mcpserver::{McpServer, Tools, streamable_http, tool::StructuredContent};
use schemars::JsonSchema;
use serde::Serialize;

use crate::analysis::SnapshotAnalysis;
use crate::app::AppState;
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::worker::queue::{Job, JobKind, JobState};

const JOB_PAGE: i64 = 50;

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectsOutput {
    projects: Vec<ProjectOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectOutput {
    project_id: String,
    organization_id: String,
    organization_name: String,
    name: String,
    remote_url: String,
    forge: String,
    description: Option<String>,
    uses_default_analyzers: bool,
    analyzers: Vec<String>,
    viewer_role: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct AnalysesOutput {
    analyses: Vec<AnalysisOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct AnalysisOutput {
    snapshot_id: String,
    subject_kind: String,
    subject_key: String,
    observed_at: String,
    head_sha: String,
    base_sha: Option<String>,
    merge_base_sha: Option<String>,
    runs: Vec<RunOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct RunOutput {
    run_id: String,
    analyzer: String,
    status: String,
    detail: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    finding_count: usize,
    signal_count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JobsOutput {
    jobs: Vec<JobOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JobOutput {
    job_id: String,
    project_id: String,
    kind: String,
    state: String,
    attempts: u64,
    last_error: Option<String>,
    available_at: String,
    created_at: String,
    finished_at: Option<String>,
}

struct ErrorMenuTools {
    state: Arc<AppState>,
    user: User,
}

#[Tools]
impl ErrorMenuTools {
    /// List projects available to the authenticated user.
    async fn list_projects(&self) -> Result<StructuredContent<ProjectsOutput>, String> {
        let summaries = Project::summaries_for(&self.state.database, &self.user)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project listing failed");
                "could not list projects".to_owned()
            })?;
        let mut output = Vec::with_capacity(summaries.len());
        for summary in summaries {
            output.push(
                self.project_output(
                    summary.project,
                    summary.organization_name,
                    summary.viewer_role,
                )
                .await?,
            );
        }

        Ok(StructuredContent(ProjectsOutput { projects: output }))
    }

    /// Read one project available to the authenticated user.
    async fn read_project(
        &self,
        project_id: String,
    ) -> Result<StructuredContent<ProjectOutput>, String> {
        let (project, role) = self.readable_project(&project_id).await?;
        let organization_name = self.organization_name(project.organization_id).await?;

        Ok(StructuredContent(
            self.project_output(project, organization_name, role)
                .await?,
        ))
    }

    /// List recorded analyses for one project available to the authenticated user.
    async fn list_analyses(
        &self,
        project_id: String,
    ) -> Result<StructuredContent<AnalysesOutput>, String> {
        let (project, _) = self.readable_project(&project_id).await?;
        let analyses = SnapshotAnalysis::for_project(&self.state.database, project.id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP analysis listing failed");
                "could not list analyses".to_owned()
            })?;

        Ok(StructuredContent(AnalysesOutput {
            analyses: analyses.into_iter().map(analysis_output).collect(),
        }))
    }

    /// List recent jobs for one available project, or for every project when the authenticated user is an administrator.
    async fn list_jobs(
        &self,
        project_id: Option<String>,
    ) -> Result<StructuredContent<JobsOutput>, String> {
        let project_id = match project_id {
            Some(project_id) => Some(self.readable_project(&project_id).await?.0.id),
            None if self.user.role == UserRole::Admin => None,
            None => {
                return Err(
                    "a project_id is required unless the user is an administrator".to_owned(),
                );
            }
        };
        let jobs = Job::recent(&self.state.database, project_id, JOB_PAGE)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP job listing failed");
                "could not list jobs".to_owned()
            })?;

        Ok(StructuredContent(JobsOutput {
            jobs: jobs.into_iter().map(job_output).collect(),
        }))
    }
}

impl ErrorMenuTools {
    async fn readable_project(&self, project_id: &str) -> Result<(Project, ProjectRole), String> {
        let project_id = project_id
            .parse::<Id<Project>>()
            .map_err(|_| "project_id is invalid".to_owned())?;
        let project = Project::load(&self.state.database, project_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project lookup failed");
                "could not read project".to_owned()
            })?
            .ok_or_else(|| "project was not found".to_owned())?;
        let role = ProjectRole::for_user(&self.state.database, &self.user, project_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project authorization failed");
                "could not read project".to_owned()
            })?
            .ok_or_else(|| "project is not available to the authenticated user".to_owned())?;

        Ok((project, role))
    }

    async fn organization_name(&self, organization_id: Id<Organization>) -> Result<String, String> {
        Organization::load(&self.state.database, organization_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP organization lookup failed");
                "could not read project".to_owned()
            })?
            .map(|organization| organization.name)
            .ok_or_else(|| "could not read project".to_owned())
    }

    async fn project_output(
        &self,
        project: Project,
        organization_name: String,
        viewer_role: ProjectRole,
    ) -> Result<ProjectOutput, String> {
        let analyzers = project
            .effective_analyzers(&self.state.database)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project analyzer lookup failed");
                "could not read project".to_owned()
            })?;

        Ok(ProjectOutput {
            project_id: project.id.encode(),
            organization_id: project.organization_id.encode(),
            organization_name,
            name: project.name,
            remote_url: project.remote.to_string(),
            forge: forge_kind(project.forge_kind).to_owned(),
            description: project.description,
            uses_default_analyzers: project.uses_default_analyzers,
            analyzers,
            viewer_role: project_role(viewer_role).to_owned(),
        })
    }
}

fn forge_kind(kind: crate::forge::ForgeKind) -> &'static str {
    match kind {
        crate::forge::ForgeKind::Auto => "auto",
        crate::forge::ForgeKind::Github => "github",
        crate::forge::ForgeKind::Gitlab => "gitlab",
        crate::forge::ForgeKind::Gitea => "gitea",
        crate::forge::ForgeKind::Forgejo => "forgejo",
    }
}

fn project_role(role: ProjectRole) -> &'static str {
    match role {
        ProjectRole::Viewer => "viewer",
        ProjectRole::Operator => "operator",
        ProjectRole::Owner => "owner",
    }
}

fn analysis_output(analysis: SnapshotAnalysis) -> AnalysisOutput {
    let (subject_kind, subject_key) = match analysis.subject.kind {
        SubjectKind::Change { number } => ("change".to_owned(), number.to_string()),
        SubjectKind::Branch { name } => ("branch".to_owned(), name),
        SubjectKind::Commit { sha } => ("commit".to_owned(), sha.to_string()),
    };

    AnalysisOutput {
        snapshot_id: analysis.snapshot.id.encode(),
        subject_kind,
        subject_key,
        observed_at: analysis.snapshot.observed_at.to_string(),
        head_sha: analysis.snapshot.head.to_string(),
        base_sha: analysis.snapshot.base.map(|sha| sha.to_string()),
        merge_base_sha: analysis.snapshot.merge_base.map(|sha| sha.to_string()),
        runs: analysis
            .runs
            .into_iter()
            .map(|record| {
                let (status, detail) = match record.run.status {
                    crate::analysis::RunStatus::Running => ("running", None),
                    crate::analysis::RunStatus::Succeeded => ("succeeded", None),
                    crate::analysis::RunStatus::Failed { message } => ("failed", Some(message)),
                    crate::analysis::RunStatus::TimedOut => ("timed_out", None),
                    crate::analysis::RunStatus::Skipped { reason } => ("skipped", Some(reason)),
                };
                RunOutput {
                    run_id: record.run.id.encode(),
                    analyzer: record.run.analyzer,
                    status: status.to_owned(),
                    detail,
                    started_at: record.run.started_at.to_string(),
                    finished_at: record.run.finished_at.map(|at| at.to_string()),
                    finding_count: record.findings.len(),
                    signal_count: record.signals.len(),
                }
            })
            .collect(),
    }
}
fn job_output(job: Job) -> JobOutput {
    JobOutput {
        job_id: job.id.encode(),
        project_id: job.project_id.encode(),
        kind: match job.kind {
            JobKind::Discover => "discover".to_owned(),
        },
        state: match job.state {
            JobState::Queued => "queued".to_owned(),
            JobState::Running => "running".to_owned(),
            JobState::Done => "done".to_owned(),
            JobState::Failed => "failed".to_owned(),
        },
        attempts: job.attempts.max(0) as u64,
        last_error: job.last_error,
        available_at: job.available_at.to_string(),
        created_at: job.created_at.to_string(),
        finished_at: job.finished_at.map(|at| at.to_string()),
    }
}

const EVENT_STREAM: &str = "text/event-stream";

/// poem-mcpserver 0.3.1 picks the response framing from the first entry of `Accept` alone,
/// and its JSON arm answers even a single request with a one-element array. A client that
/// reads one response object, as the Streamable HTTP transport is specified to, rejects
/// that as malformed. The crate's event-stream arm is correct, so a client that offered
/// `text/event-stream` anywhere is given it by naming it first. A client that never
/// offered it keeps the framing it asked for.
async fn prefer_event_stream(mut request: Request) -> poem::Result<Request> {
    if request
        .header(header::ACCEPT)
        .is_some_and(|accept| accept.contains(EVENT_STREAM))
    {
        request.headers_mut().insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream, application/json"),
        );
    }

    Ok(request)
}

pub fn endpoint(state: Arc<AppState>) -> impl IntoEndpoint {
    streamable_http::endpoint(move |request: &Request| {
        let user = request
            .extensions()
            .get::<CurrentUser>()
            .expect("RequireSession must authenticate MCP requests")
            .0
            .clone();
        McpServer::new()
            .tools(ErrorMenuTools {
                state: Arc::clone(&state),
                user,
            })
            .with_server_info("error.menu", env!("CARGO_PKG_VERSION"))
    })
    .into_endpoint()
    .before(prefer_event_stream)
}

#[cfg(test)]
mod tests {
    use poem::http::{Method, StatusCode, Uri};
    use poem::{Endpoint, Response};
    use poem_mcpserver::content::Text;

    use super::*;

    struct ProbeTools;

    #[Tools]
    impl ProbeTools {
        /// Exists so the probe server has one tool to list.
        async fn ping(&self) -> Text<String> {
            Text("pong".to_owned())
        }
    }

    fn server() -> impl IntoEndpoint {
        streamable_http::endpoint(|_: &Request| {
            McpServer::new()
                .tools(ProbeTools)
                .with_server_info("probe", "0")
        })
    }

    fn probe() -> impl Endpoint<Output = Response> {
        server()
            .into_endpoint()
            .before(prefer_event_stream)
            .map_to_response()
    }

    async fn session_of(endpoint: &impl Endpoint<Output = Response>) -> String {
        let initialize = endpoint
            .call(post(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}"#,
                None,
            ))
            .await
            .expect("an initialize response");
        assert_eq!(initialize.status(), StatusCode::OK);

        initialize
            .headers()
            .get("Mcp-Session-Id")
            .expect("a session")
            .to_str()
            .expect("a readable session")
            .to_owned()
    }

    /// The `Accept` order every Streamable HTTP client sends, and the one that selects the
    /// arm of poem-mcpserver that answers a single request with an array.
    fn post(body: &'static str, session: Option<&str>) -> Request {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(Uri::from_static("/"))
            .content_type("application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(session) = session {
            builder = builder.header("Mcp-Session-Id", session);
        }

        builder.body(body)
    }

    #[tokio::test]
    async fn a_call_answers_one_response_and_not_an_array_of_one() {
        let endpoint = probe();
        let session = session_of(&endpoint).await;

        let listed = endpoint
            .call(post(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                Some(&session),
            ))
            .await
            .unwrap();
        assert!(
            listed
                .content_type()
                .is_some_and(|content_type| content_type.starts_with(EVENT_STREAM)),
            "framing must be the arm that answers one response per event"
        );

        let body = listed.into_body().into_string().await.expect("a body");
        let frames = body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 1, "one request, one response: {body}");

        let response: serde_json::Value =
            serde_json::from_str(frames[0].trim()).expect("a JSON-RPC response");
        assert!(response.is_object(), "not an array of one: {body}");
        assert_eq!(response["id"], 2);
    }

    /// Why [`prefer_event_stream`] exists. When this fails, poem-mcpserver has learned to
    /// answer a single request with a single object and the nudge can be deleted.
    #[tokio::test]
    async fn the_json_arm_still_answers_a_single_request_with_an_array() {
        let endpoint = server().into_endpoint().map_to_response();
        let session = session_of(&endpoint).await;

        let listed = endpoint
            .call(post(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                Some(&session),
            ))
            .await
            .unwrap();
        let body = listed.into_body().into_string().await.expect("a body");
        let response: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");

        assert!(response.is_array(), "the crate was fixed upstream: {body}");
    }
}
