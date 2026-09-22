//! The HTTP surface, one module per resource.
//!
//! A response enum lists every status an operation answers with, including the 401 the
//! session middleware returns before a handler runs and the 403 a bearer token gets on a
//! write. Those variants are never constructed here: they are what the generated client
//! reads out of the document, so the enums that carry them allow dead code.

pub mod analysis;
pub mod health;
pub mod job;
pub mod member;
pub mod organization;
pub mod project;
pub mod repository;
pub mod token;
pub mod user;

use poem_openapi::Object;

use crate::app::AppState;
use crate::http::api::user::{UserOutput, UserRoleOutput};
use crate::http::auth;
use crate::prelude::*;

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct Error {
    pub message: String,
}

pub enum ProjectPermission {
    Viewer,
    Operator,
    Owner,
}

pub enum ProjectAccess {
    Allowed { project: Project, role: ProjectRole },
    Forbidden,
    Missing,
    Failed(String),
}

pub async fn project_access(
    state: &AppState,
    user: &User,
    project_id: Id<Project>,
    permission: ProjectPermission,
) -> ProjectAccess {
    let project = match Project::load(&state.database, project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return ProjectAccess::Missing,
        Err(error) => return ProjectAccess::Failed(error.to_string()),
    };
    let role = match ProjectRole::for_user(&state.database, user, project_id).await {
        Ok(role) => role,
        Err(error) => return ProjectAccess::Failed(error.to_string()),
    };
    let allowed = matches!(
        (role, permission),
        (Some(_), ProjectPermission::Viewer)
            | (
                Some(ProjectRole::Operator | ProjectRole::Owner),
                ProjectPermission::Operator
            )
            | (Some(ProjectRole::Owner), ProjectPermission::Owner)
    );
    if let Some(role) = role.filter(|_| allowed) {
        ProjectAccess::Allowed { project, role }
    } else {
        ProjectAccess::Forbidden
    }
}

pub enum OrganizationPermission {
    Viewer,
    Operator,
    Owner,
}

pub enum OrganizationAccess {
    Allowed {
        organization: Organization,
        role: OrganizationRole,
    },
    Forbidden,
    Missing,
    Failed(String),
}

pub async fn organization_access(
    state: &AppState,
    user: &User,
    organization_id: Id<Organization>,
    permission: OrganizationPermission,
) -> OrganizationAccess {
    let organization = match Organization::load(&state.database, organization_id).await {
        Ok(Some(organization)) => organization,
        Ok(None) => return OrganizationAccess::Missing,
        Err(error) => return OrganizationAccess::Failed(error.to_string()),
    };
    let role = match OrganizationRole::for_user(&state.database, user, organization_id).await {
        Ok(role) => role,
        Err(error) => return OrganizationAccess::Failed(error.to_string()),
    };
    let allowed = matches!(
        (role, permission),
        (Some(_), OrganizationPermission::Viewer)
            | (
                Some(OrganizationRole::Operator | OrganizationRole::Owner),
                OrganizationPermission::Operator
            )
            | (Some(OrganizationRole::Owner), OrganizationPermission::Owner)
    );
    if let Some(role) = role.filter(|_| allowed) {
        OrganizationAccess::Allowed { organization, role }
    } else {
        OrganizationAccess::Forbidden
    }
}

/// Who may change a project's custody: move it to another organization, or delete it.
///
/// Both need an owner of the organization that holds the project, never an owner of the
/// project alone. A project grant is given out one repository at a time, and anyone who
/// may create an organization could otherwise walk a project out of the one that owns it.
pub enum CustodyAccess {
    Allowed {
        project: Project,
        organization: Organization,
    },
    Forbidden,
    Missing,
    Failed(String),
}

pub async fn custody_access(
    state: &AppState,
    user: &User,
    project_id: Id<Project>,
) -> CustodyAccess {
    let project = match Project::load(&state.database, project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return CustodyAccess::Missing,
        Err(error) => return CustodyAccess::Failed(error.to_string()),
    };
    match organization_access(
        state,
        user,
        project.organization_id,
        OrganizationPermission::Owner,
    )
    .await
    {
        OrganizationAccess::Allowed { organization, .. } => CustodyAccess::Allowed {
            project,
            organization,
        },
        OrganizationAccess::Forbidden => CustodyAccess::Forbidden,
        OrganizationAccess::Missing => CustodyAccess::Failed(format!(
            "project {} names an organization that does not exist",
            project.id.encode()
        )),
        OrganizationAccess::Failed(message) => CustodyAccess::Failed(message),
    }
}

pub fn can_create_organization(user: &User) -> bool {
    matches!(user.role, UserRole::Member | UserRole::Admin)
}

pub fn user_output(user: User) -> UserOutput {
    UserOutput {
        user_id: user.id.encode(),
        display_name: user.display_name,
        role: UserRoleOutput::from_role(user.role),
    }
}

pub fn forbidden() -> Error {
    Error {
        message: "the current user does not have the required role".to_owned(),
    }
}

pub fn missing_project() -> Error {
    Error {
        message: "project was not found".to_owned(),
    }
}

pub fn missing_organization() -> Error {
    Error {
        message: "organization was not found".to_owned(),
    }
}

pub fn missing_user() -> Error {
    Error {
        message: "user was not found".to_owned(),
    }
}

pub fn api_token_error(operation: &'static str, error: DatabaseError) -> Error {
    auth::log_database_error(operation, &error);
    Error {
        message: "API token storage operation failed".to_owned(),
    }
}
