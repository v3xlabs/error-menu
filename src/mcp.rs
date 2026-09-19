use std::sync::Arc;

use poem::{IntoEndpoint, Request};
use poem_mcpserver::{McpServer, Tools, streamable_http, tool::StructuredContent};
use schemars::JsonSchema;
use serde::Serialize;

use crate::auth::CurrentUser;
use crate::id::Id;
use crate::queue::{JobKind, JobRecord, JobState};
use crate::store::{AnalysisRecord, Store};
use crate::user::{ProjectRole, User, UserRole};
use crate::watch::{Project, SubjectKind};

const JOB_PAGE: i64 = 50;

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectsOutput {
    projects: Vec<ProjectOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectOutput {
    project_id: String,
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
    store: Arc<Store>,
    user: User,
}

#[Tools]
impl ErrorMenuTools {
    /// List projects available to the authenticated user.
    async fn list_projects(&self) -> Result<StructuredContent<ProjectsOutput>, String> {
        let projects = self
            .store
            .list_projects_for(&self.user)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project listing failed");
                "could not list projects".to_owned()
            })?;
        let mut output = Vec::with_capacity(projects.len());
        for project in projects {
            let role = self
                .store
                .project_role_for(&self.user, project.id)
                .await
                .map_err(|error| {
                    tracing::error!(%error, "MCP project authorization failed");
                    "could not list projects".to_owned()
                })?
                .ok_or_else(|| "could not list projects".to_owned())?;
            output.push(self.project_output(project, role).await?);
        }

        Ok(StructuredContent(ProjectsOutput { projects: output }))
    }

    /// Read one project available to the authenticated user.
    async fn read_project(
        &self,
        project_id: String,
    ) -> Result<StructuredContent<ProjectOutput>, String> {
        let (project, role) = self.readable_project(&project_id).await?;
        Ok(StructuredContent(self.project_output(project, role).await?))
    }

    /// List recorded analyses for one project available to the authenticated user.
    async fn list_analyses(
        &self,
        project_id: String,
    ) -> Result<StructuredContent<AnalysesOutput>, String> {
        let (project, _) = self.readable_project(&project_id).await?;
        let analyses = self
            .store
            .analyses_for_project(project.id)
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
        let jobs = self
            .store
            .recent_jobs(project_id, JOB_PAGE)
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
        let project = self
            .store
            .project(project_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project lookup failed");
                "could not read project".to_owned()
            })?
            .ok_or_else(|| "project was not found".to_owned())?;
        let role = self
            .store
            .project_role_for(&self.user, project_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project authorization failed");
                "could not read project".to_owned()
            })?
            .ok_or_else(|| "project is not available to the authenticated user".to_owned())?;

        Ok((project, role))
    }

    async fn project_output(
        &self,
        project: Project,
        viewer_role: ProjectRole,
    ) -> Result<ProjectOutput, String> {
        let analyzers = self
            .store
            .effective_project_analyzers(&project)
            .await
            .map_err(|error| {
                tracing::error!(%error, "MCP project analyzer lookup failed");
                "could not read project".to_owned()
            })?;

        Ok(ProjectOutput {
            project_id: project.id.encode(),
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

fn analysis_output(analysis: AnalysisRecord) -> AnalysisOutput {
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
fn job_output(job: JobRecord) -> JobOutput {
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

pub fn endpoint(store: Arc<Store>) -> impl IntoEndpoint {
    streamable_http::endpoint(move |request: &Request| {
        let user = request
            .extensions()
            .get::<CurrentUser>()
            .expect("RequireSession must authenticate MCP requests")
            .0
            .clone();
        McpServer::new()
            .tools(ErrorMenuTools {
                store: Arc::clone(&store),
                user,
            })
            .with_server_info("error.menu", env!("CARGO_PKG_VERSION"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::test::TestClient;
    use poem_mcpserver::protocol::{
        JSON_RPC_VERSION,
        rpc::{Request as McpRequest, RequestId, Requests},
        tool::{ToolsCallRequest, ToolsListRequest},
    };

    #[tokio::test]
    async fn tools_are_read_only_and_projects_are_authorized() {
        let store = Arc::new(
            Store::open("sqlite::memory:", 0)
                .await
                .expect("store opens"),
        );
        let owner = store
            .register_user("https://github.com", "owner", "Owner", true)
            .await
            .expect("owner registers")
            .expect("owner is allowed");
        let member = store
            .register_user("https://github.com", "member", "Member", true)
            .await
            .expect("member registers")
            .expect("member is allowed");
        store
            .set_user_role(member.id, UserRole::Member)
            .await
            .expect("member role updates");
        store
            .create_project(
                owner.id,
                "private",
                crate::vcs::RemoteUrl::new("https://example.invalid/private").expect("URL parses"),
                crate::forge::ForgeKind::Auto,
                true,
                &[],
            )
            .await
            .expect("project creates");

        let mut server = McpServer::new().tools(ErrorMenuTools {
            store,
            user: member,
        });
        let response = server
            .handle_request(McpRequest {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                id: Some(RequestId::Int(1)),
                body: Requests::ToolsList {
                    params: ToolsListRequest { cursor: None },
                },
            })
            .await;
        let response = serde_json::to_value(response).expect("tool response serializes");
        let names = response["result"]["tools"]
            .as_array()
            .expect("tool list is present")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_projects",
                "read_project",
                "list_analyses",
                "list_jobs"
            ]
        );

        let response = server
            .handle_request(McpRequest {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                id: Some(RequestId::Int(2)),
                body: Requests::ToolsCall {
                    params: ToolsCallRequest {
                        name: "list_projects".to_owned(),
                        arguments: serde_json::json!({}),
                    },
                },
            })
            .await;
        let response = serde_json::to_value(response).expect("tool response serializes");
        assert!(
            response["result"]["structuredContent"]["projects"]
                .as_array()
                .expect("projects are present")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn streamable_http_accepts_bearer_authentication() {
        let store = Arc::new(
            Store::open("sqlite::memory:", 0)
                .await
                .expect("store opens"),
        );
        let user = store
            .register_user("https://github.com", "owner", "Owner", true)
            .await
            .expect("user registers")
            .expect("user is allowed");
        let token = "mcp-test-token";
        store
            .create_api_token(
                user.id,
                &blake3::hash(token.as_bytes()).to_hex().to_string(),
                "mcp-test",
                None,
            )
            .await
            .expect("token creates");
        let response = TestClient::new(crate::http::routes(
            store,
            std::path::PathBuf::from(".tmp/analysis/test"),
            None,
        ))
        .post("/mcp")
        .header("accept", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body_json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}
            }
        }))
        .send()
        .await;

        response.assert_status_is_ok();
        response.assert_header_exist("mcp-session-id");
    }
}
