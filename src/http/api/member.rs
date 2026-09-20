use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::app::AppState;
use crate::http::api::{
    Error, ProjectAccess, ProjectPermission, forbidden, missing_project, missing_user,
    project_access,
};
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::project::member::ProjectMemberChange;

pub struct MemberApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl MemberApi {
    #[oai(path = "/projects/:project_id/members", method = "get")]
    async fn list_project_members(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> ProjectMembersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectMembersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectMembersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectMembersResponse::Failed(Json(Error { message }));
            }
        }
        match ProjectMember::list(&self.state.database, project_id).await {
            Ok(members) => ProjectMembersResponse::Found(Json(ProjectMembersOutput {
                members: members.into_iter().map(project_member_output).collect(),
            })),
            Err(error) => ProjectMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id/members/:user_id", method = "put")]
    async fn set_project_member(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        user_id: Path<String>,
        input: Json<SetProjectMember>,
    ) -> ProjectMembersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectMembersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectMembersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectMembersResponse::Failed(Json(Error { message }));
            }
        }
        let user_id = match user_id.0.parse::<Id<User>>() {
            Ok(user_id) => user_id,
            Err(_) => return ProjectMembersResponse::Missing(Json(missing_user())),
        };
        match User::load(&self.state.database, user_id).await {
            Ok(Some(member)) if member.role != UserRole::Guest => {}
            Ok(Some(_)) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: "a guest must become a member before joining a project".to_owned(),
                }));
            }
            Ok(None) => return ProjectMembersResponse::Missing(Json(missing_user())),
            Err(error) => {
                return ProjectMembersResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        }
        match ProjectMember::set(
            &self.state.database,
            project_id,
            user_id,
            input.0.role.into_role(),
        )
        .await
        {
            Ok(ProjectMemberChange::Updated) => {
                match ProjectMember::list(&self.state.database, project_id).await {
                    Ok(members) => ProjectMembersResponse::Found(Json(ProjectMembersOutput {
                        members: members.into_iter().map(project_member_output).collect(),
                    })),
                    Err(error) => ProjectMembersResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
            Ok(ProjectMemberChange::FinalOwner) => ProjectMembersResponse::Invalid(Json(Error {
                message: "a project must keep an owner".to_owned(),
            })),
            Ok(ProjectMemberChange::Missing | ProjectMemberChange::Removed) => {
                ProjectMembersResponse::Missing(Json(missing_user()))
            }
            Err(error) => ProjectMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id/members/:user_id", method = "delete")]
    async fn remove_project_member(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        user_id: Path<String>,
    ) -> ProjectMembersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectMembersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectMembersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectMembersResponse::Failed(Json(Error { message }));
            }
        }
        let user_id = match user_id.0.parse::<Id<User>>() {
            Ok(user_id) => user_id,
            Err(_) => return ProjectMembersResponse::Missing(Json(missing_user())),
        };
        match ProjectMember::remove(&self.state.database, project_id, user_id).await {
            Ok(ProjectMemberChange::Removed) => {
                match ProjectMember::list(&self.state.database, project_id).await {
                    Ok(members) => ProjectMembersResponse::Found(Json(ProjectMembersOutput {
                        members: members.into_iter().map(project_member_output).collect(),
                    })),
                    Err(error) => ProjectMembersResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
            Ok(ProjectMemberChange::Missing) => {
                ProjectMembersResponse::Missing(Json(missing_user()))
            }
            Ok(ProjectMemberChange::FinalOwner) => ProjectMembersResponse::Invalid(Json(Error {
                message: "a project must keep an owner".to_owned(),
            })),
            Ok(ProjectMemberChange::Updated) => ProjectMembersResponse::Failed(Json(Error {
                message: "project membership state was invalid".to_owned(),
            })),
            Err(error) => ProjectMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
pub enum ProjectRoleOutput {
    Viewer,
    Operator,
    Owner,
}

impl ProjectRoleOutput {
    pub fn from_role(role: ProjectRole) -> Self {
        match role {
            ProjectRole::Viewer => Self::Viewer,
            ProjectRole::Operator => Self::Operator,
            ProjectRole::Owner => Self::Owner,
        }
    }

    fn into_role(self) -> ProjectRole {
        match self {
            Self::Viewer => ProjectRole::Viewer,
            Self::Operator => ProjectRole::Operator,
            Self::Owner => ProjectRole::Owner,
        }
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ProjectMemberOutput {
    user_id: String,
    display_name: String,
    role: ProjectRoleOutput,
}

#[derive(Debug, Object)]
struct ProjectMembersOutput {
    members: Vec<ProjectMemberOutput>,
}

#[derive(Debug, Object)]
struct SetProjectMember {
    role: ProjectRoleOutput,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ProjectMembersResponse {
    #[oai(status = 200)]
    Found(Json<ProjectMembersOutput>),
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

fn project_member_output(member: ProjectMember) -> ProjectMemberOutput {
    ProjectMemberOutput {
        user_id: member.user_id.encode(),
        display_name: member.display_name,
        role: ProjectRoleOutput::from_role(member.role),
    }
}
