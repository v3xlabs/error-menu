use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::app::AppState;
use crate::http::api::project::{GetProjectResponse, project_response};
use crate::http::api::{
    Error, ProjectAccess, ProjectPermission, forbidden, missing_project, project_access,
};
use crate::http::auth::CurrentUser;
use crate::http::icon::is_image_path;
use crate::prelude::*;
use crate::vcs::mirror::Mirror;

pub struct RepositoryApi {
    pub state: Arc<AppState>,
}

/// Enough history to see the shape of recent work without turning a project page into a
/// commit log. Every commit read costs one object load out of the mirror.
const DEFAULT_COMMITS: u8 = 10;
const MAX_COMMITS: u8 = 50;

#[OpenApi]
impl RepositoryApi {
    /// One directory of the repository at the default branch, for browsing it. Directories
    /// come first, so the list reads the way a file manager does.
    #[oai(path = "/projects/:project_id/tree", method = "get")]
    async fn project_tree(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        path: poem_openapi::param::Query<Option<String>>,
    ) -> TreeResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return TreeResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Viewer).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => return TreeResponse::Forbidden(Json(forbidden())),
                ProjectAccess::Missing => return TreeResponse::Missing(Json(missing_project())),
                ProjectAccess::Failed(message) => {
                    return TreeResponse::Failed(Json(Error { message }));
                }
            };
        let head = match Snapshot::default_branch_head(&self.state.database, project_id).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                return TreeResponse::Missing(Json(Error {
                    message: "run discovery first, so the default branch is known".to_owned(),
                }));
            }
            Err(error) => {
                return TreeResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mirror = match Mirror::open(&self.state.mirrors, &project.remote).await {
            Ok(mirror) => mirror,
            Err(error) => {
                return TreeResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let directory = path.0.unwrap_or_default();
        match mirror.entries_at(&head, &directory).await {
            Ok(entries) => TreeResponse::Found(Json(TreeOutput {
                path: directory,
                entries: entries.into_iter().map(entry_output).collect(),
            })),
            Err(error) => TreeResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// The default branch's own history, newest first, read from the mirror. A forge is
    /// never asked for this: git already carries it, and a reader who opens a project
    /// wants to see what landed before they read what it means.
    #[oai(path = "/projects/:project_id/commits", method = "get")]
    async fn project_commits(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        limit: poem_openapi::param::Query<Option<u8>>,
    ) -> CommitsResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return CommitsResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Viewer).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => return CommitsResponse::Forbidden(Json(forbidden())),
                ProjectAccess::Missing => return CommitsResponse::Missing(Json(missing_project())),
                ProjectAccess::Failed(message) => {
                    return CommitsResponse::Failed(Json(Error { message }));
                }
            };
        let head = match Snapshot::default_branch_head(&self.state.database, project_id).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                return CommitsResponse::Missing(Json(Error {
                    message: "run discovery first, so the default branch is known".to_owned(),
                }));
            }
            Err(error) => {
                return CommitsResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mirror = match Mirror::open(&self.state.mirrors, &project.remote).await {
            Ok(mirror) => mirror,
            Err(error) => {
                return CommitsResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let limit = usize::from(limit.0.unwrap_or(DEFAULT_COMMITS).clamp(1, MAX_COMMITS));

        match mirror.log(&head, limit).await {
            Ok(commits) => CommitsResponse::Found(Json(CommitsOutput {
                commits: commits.into_iter().map(commit_output).collect(),
            })),
            Err(error) => CommitsResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// Every image in the repository that could be the project's mark, best first. The
    /// ranking is a guess, so the whole list is offered and the reader decides.
    #[oai(path = "/projects/:project_id/icon-candidates", method = "get")]
    async fn icon_candidates(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> IconCandidatesResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return IconCandidatesResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Viewer).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => {
                    return IconCandidatesResponse::Forbidden(Json(forbidden()));
                }
                ProjectAccess::Missing => {
                    return IconCandidatesResponse::Missing(Json(missing_project()));
                }
                ProjectAccess::Failed(message) => {
                    return IconCandidatesResponse::Failed(Json(Error { message }));
                }
            };
        let head = match Snapshot::default_branch_head(&self.state.database, project_id).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                return IconCandidatesResponse::Missing(Json(Error {
                    message: "run discovery first, so the default branch is known".to_owned(),
                }));
            }
            Err(error) => {
                return IconCandidatesResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mirror = match Mirror::open(&self.state.mirrors, &project.remote).await {
            Ok(mirror) => mirror,
            Err(error) => {
                return IconCandidatesResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match crate::project::icon::suggest(&mirror, &head, &project.name).await {
            Ok(candidates) => IconCandidatesResponse::Found(Json(IconCandidatesOutput {
                candidates: candidates.into_iter().map(candidate_output).collect(),
            })),
            Err(error) => IconCandidatesResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// Sets the mark by hand. A path is kept rather than the bytes, so the picture follows
    /// the default branch instead of going stale.
    #[oai(path = "/projects/:project_id/icon", method = "put")]
    async fn set_project_icon(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<SetProjectIcon>,
    ) -> GetProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => {
                    return GetProjectResponse::Forbidden(Json(forbidden()));
                }
                ProjectAccess::Missing => {
                    return GetProjectResponse::Missing(Json(missing_project()));
                }
                ProjectAccess::Failed(message) => {
                    return GetProjectResponse::Failed(Json(Error { message }));
                }
            };
        let icon = match icon_from_input(&input.0) {
            Ok(icon) => icon,
            Err(message) => return GetProjectResponse::Invalid(Json(Error { message })),
        };
        if let Err(error) = project
            .describe(&self.state.database, project.description.as_deref(), &icon)
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }

        project_response(&self.state.database, project_id, ProjectRole::Owner).await
    }
}

/// A file or directory in the repository. `is_image` says whether it could be shown as a
/// mark, so the browser can grey out the rest without fetching anything.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct TreeEntryOutput {
    name: String,
    path: String,
    is_directory: bool,
    is_image: bool,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct TreeOutput {
    path: String,
    entries: Vec<TreeEntryOutput>,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum TreeResponse {
    #[oai(status = 200)]
    Found(Json<TreeOutput>),
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

/// One commit of the default branch. `summary` is the first line of the commit message,
/// and every field here is written by whoever made the commit, so it is a claim rather
/// than a verified fact.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct CommitOutput {
    sha: String,
    summary: String,
    author: Option<String>,
    authored_at: String,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct CommitsOutput {
    commits: Vec<CommitOutput>,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum CommitsResponse {
    #[oai(status = 200)]
    Found(Json<CommitsOutput>),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum SchemeOutput {
    Light,
    Dark,
    Either,
}

/// One image in the repository that could be the project's mark. `scheme` is what the file
/// name says it is drawn for, and `score` is only a ranking, not a measurement.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct IconCandidateOutput {
    path: String,
    scheme: SchemeOutput,
    score: i32,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct IconCandidatesOutput {
    candidates: Vec<IconCandidateOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SetProjectIcon {
    light_path: Option<String>,
    dark_path: Option<String>,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum IconCandidatesResponse {
    #[oai(status = 200)]
    Found(Json<IconCandidatesOutput>),
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

fn entry_output(entry: crate::vcs::mirror::TreeEntry) -> TreeEntryOutput {
    TreeEntryOutput {
        is_image: !entry.is_directory && is_image_path(entry.path.as_str()),
        name: entry.name,
        path: entry.path.as_str().to_owned(),
        is_directory: entry.is_directory,
    }
}

fn commit_output(commit: crate::vcs::mirror::LoggedCommit) -> CommitOutput {
    CommitOutput {
        sha: commit.sha.to_string(),
        summary: commit.summary,
        author: commit.author,
        authored_at: commit.authored_at.to_string(),
    }
}

fn candidate_output(candidate: crate::project::icon::Candidate) -> IconCandidateOutput {
    IconCandidateOutput {
        path: candidate.path.as_str().to_owned(),
        scheme: match candidate.scheme {
            crate::project::icon::Scheme::Light => SchemeOutput::Light,
            crate::project::icon::Scheme::Dark => SchemeOutput::Dark,
            crate::project::icon::Scheme::Either => SchemeOutput::Either,
        },
        score: candidate.score,
    }
}

/// A path arrives from the reader, so it is parsed before it reaches the repository.
fn icon_from_input(input: &SetProjectIcon) -> Result<ProjectIcon, String> {
    let parse = |path: &Option<String>| -> Result<Option<RepoPath>, String> {
        path.as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(|path| RepoPath::new(path).map_err(|_| format!("{path} is not a usable path")))
            .transpose()
    };

    Ok(ProjectIcon {
        light: parse(&input.light_path)?,
        dark: parse(&input.dark_path)?,
    })
}
