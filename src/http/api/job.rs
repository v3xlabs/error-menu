use std::sync::Arc;

use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::app::AppState;
use crate::http::api::{
    Error, ProjectAccess, ProjectPermission, forbidden, missing_project, project_access,
};
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::worker::queue::Job;

const JOB_PAGE: i64 = 50;

pub struct JobApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl JobApi {
    /// What the queue holds, newest first. The schedule runs without anybody watching, so
    /// this is how a reader finds out that it did, or that it tried and failed.
    #[oai(path = "/jobs", method = "get")]
    async fn jobs(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: poem_openapi::param::Query<Option<String>>,
    ) -> ListJobsResponse {
        let project_id = match project_id
            .0
            .as_deref()
            .map(str::parse::<Id<Project>>)
            .transpose()
        {
            Ok(project_id) => project_id,
            Err(error) => {
                return ListJobsResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_id {
            Some(project_id) => {
                match project_access(&self.state, &user, project_id, ProjectPermission::Viewer)
                    .await
                {
                    ProjectAccess::Allowed { .. } => {}
                    ProjectAccess::Forbidden => {
                        return ListJobsResponse::Forbidden(Json(forbidden()));
                    }
                    ProjectAccess::Missing => {
                        return ListJobsResponse::Missing(Json(missing_project()));
                    }
                    ProjectAccess::Failed(message) => {
                        return ListJobsResponse::Failed(Json(Error { message }));
                    }
                }
            }
            None if user.role != UserRole::Admin => {
                return ListJobsResponse::Forbidden(Json(forbidden()));
            }
            None => {}
        }

        match Job::recent(&self.state.database, project_id, JOB_PAGE).await {
            Ok(jobs) => ListJobsResponse::Found(Json(JobsOutput {
                jobs: jobs.into_iter().map(job_output).collect(),
            })),
            Err(error) => ListJobsResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

/// What the queue is doing, for a reader who wants to know that scanning happens without
/// them. A job carries its own error, because "nothing has run" and "it tried and the forge
/// refused" are different answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum JobKindOutput {
    Discover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum JobStateOutput {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct JobOutput {
    job_id: String,
    project_id: String,
    kind: JobKindOutput,
    state: JobStateOutput,
    attempts: u32,
    last_error: Option<String>,
    available_at: String,
    created_at: String,
    finished_at: Option<String>,
}

#[derive(Debug, Object)]
struct JobsOutput {
    jobs: Vec<JobOutput>,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ListJobsResponse {
    #[oai(status = 200)]
    Found(Json<JobsOutput>),
    #[oai(status = 400)]
    Invalid(Json<Error>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 403)]
    Forbidden(Json<Error>),
    #[oai(status = 404)]
    Missing(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}

fn job_output(job: Job) -> JobOutput {
    JobOutput {
        job_id: job.id.encode(),
        project_id: job.project_id.encode(),
        kind: match job.kind {
            crate::worker::queue::JobKind::Discover => JobKindOutput::Discover,
        },
        state: match job.state {
            crate::worker::queue::JobState::Queued => JobStateOutput::Queued,
            crate::worker::queue::JobState::Running => JobStateOutput::Running,
            crate::worker::queue::JobState::Done => JobStateOutput::Done,
            crate::worker::queue::JobState::Failed => JobStateOutput::Failed,
        },
        attempts: job.attempts.max(0) as u32,
        last_error: job.last_error,
        available_at: job.available_at.to_string(),
        created_at: job.created_at.to_string(),
        finished_at: job.finished_at.map(|at| at.to_string()),
    }
}
