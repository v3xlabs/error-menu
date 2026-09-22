use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::app::AppState;
use crate::http::api::{
    Error, OrganizationAccess, OrganizationPermission, can_create_organization, forbidden,
    missing_organization, missing_user, organization_access,
};
use crate::http::auth::CurrentUser;
use crate::organization::member::OrganizationMemberChange;
use crate::organization::{OrganizationDeletion, OrganizationSummary};
use crate::prelude::*;

pub struct OrganizationApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl OrganizationApi {
    #[oai(path = "/orgs", method = "get")]
    async fn list_organizations(
        &self,
        CurrentUser(user): CurrentUser,
    ) -> ListOrganizationsResponse {
        match Organization::summaries_for(&self.state.database, &user).await {
            Ok(summaries) => ListOrganizationsResponse::Found(Json(OrganizationsOutput {
                organizations: summaries.into_iter().map(organization_output).collect(),
            })),
            Err(error) => ListOrganizationsResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/orgs", method = "post")]
    async fn create_organization(
        &self,
        CurrentUser(user): CurrentUser,
        input: Json<CreateOrganization>,
    ) -> CreateOrganizationResponse {
        if !can_create_organization(&user) {
            return CreateOrganizationResponse::Forbidden(Json(forbidden()));
        }
        let input = input.0;
        let name = input.name.trim();
        if name.is_empty() {
            return CreateOrganizationResponse::Invalid(Json(Error {
                message: "organization name is empty".to_owned(),
            }));
        }
        let description = input
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty());

        match Organization::create(&self.state.database, user.id, name, description).await {
            Ok(organization) => CreateOrganizationResponse::Created(Json(OrganizationOutput {
                organization_id: organization.id.encode(),
                name: organization.name,
                description: organization.description,
                viewer_role: OrganizationRoleOutput::Owner,
                project_count: 0,
            })),
            Err(error) => CreateOrganizationResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/orgs/:organization_id", method = "put")]
    async fn describe_organization(
        &self,
        CurrentUser(user): CurrentUser,
        organization_id: Path<String>,
        input: Json<CreateOrganization>,
    ) -> OrganizationResponse {
        let organization_id = match organization_id.0.parse::<Id<Organization>>() {
            Ok(organization_id) => organization_id,
            Err(error) => {
                return OrganizationResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let organization = match organization_access(
            &self.state,
            &user,
            organization_id,
            OrganizationPermission::Owner,
        )
        .await
        {
            OrganizationAccess::Allowed { organization, .. } => organization,
            OrganizationAccess::Forbidden => {
                return OrganizationResponse::Forbidden(Json(forbidden()));
            }
            OrganizationAccess::Missing => {
                return OrganizationResponse::Missing(Json(missing_organization()));
            }
            OrganizationAccess::Failed(message) => {
                return OrganizationResponse::Failed(Json(Error { message }));
            }
        };
        let input = input.0;
        let name = input.name.trim();
        if name.is_empty() {
            return OrganizationResponse::Invalid(Json(Error {
                message: "organization name is empty".to_owned(),
            }));
        }
        let description = input
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty());

        match organization
            .describe(&self.state.database, name, description)
            .await
        {
            Ok(()) => self.one_organization(&user, organization_id).await,
            Err(error) => OrganizationResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/orgs/:organization_id", method = "get")]
    async fn organization(
        &self,
        CurrentUser(user): CurrentUser,
        organization_id: Path<String>,
    ) -> OrganizationResponse {
        let organization_id = match organization_id.0.parse::<Id<Organization>>() {
            Ok(organization_id) => organization_id,
            Err(error) => {
                return OrganizationResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match organization_access(
            &self.state,
            &user,
            organization_id,
            OrganizationPermission::Viewer,
        )
        .await
        {
            OrganizationAccess::Allowed { .. } => {
                self.one_organization(&user, organization_id).await
            }
            OrganizationAccess::Forbidden => OrganizationResponse::Forbidden(Json(forbidden())),
            OrganizationAccess::Missing => {
                OrganizationResponse::Missing(Json(missing_organization()))
            }
            OrganizationAccess::Failed(message) => {
                OrganizationResponse::Failed(Json(Error { message }))
            }
        }
    }

    #[oai(path = "/orgs/:organization_id/members", method = "get")]
    async fn list_organization_members(
        &self,
        CurrentUser(user): CurrentUser,
        organization_id: Path<String>,
    ) -> OrganizationMembersResponse {
        let organization_id = match self.owned_organization(&user, &organization_id.0).await {
            Ok(organization_id) => organization_id,
            Err(response) => return response,
        };

        self.members(organization_id).await
    }

    #[oai(path = "/orgs/:organization_id/members/:user_id", method = "put")]
    async fn set_organization_member(
        &self,
        CurrentUser(user): CurrentUser,
        organization_id: Path<String>,
        user_id: Path<String>,
        input: Json<SetOrganizationMember>,
    ) -> OrganizationMembersResponse {
        let organization_id = match self.owned_organization(&user, &organization_id.0).await {
            Ok(organization_id) => organization_id,
            Err(response) => return response,
        };
        let member_id = match user_id.0.parse::<Id<User>>() {
            Ok(member_id) => member_id,
            Err(_) => return OrganizationMembersResponse::Missing(Json(missing_user())),
        };
        match User::load(&self.state.database, member_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return OrganizationMembersResponse::Missing(Json(missing_user())),
            Err(error) => {
                return OrganizationMembersResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        }
        match OrganizationMember::set(
            &self.state.database,
            organization_id,
            member_id,
            input.0.role.into_role(),
        )
        .await
        {
            Ok(OrganizationMemberChange::Updated) => self.members(organization_id).await,
            Ok(OrganizationMemberChange::FinalOwner) => {
                OrganizationMembersResponse::Invalid(Json(Error {
                    message: "an organization must keep an owner".to_owned(),
                }))
            }
            Ok(OrganizationMemberChange::Missing | OrganizationMemberChange::Removed) => {
                OrganizationMembersResponse::Missing(Json(missing_user()))
            }
            Err(error) => OrganizationMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/orgs/:organization_id/members/:user_id", method = "delete")]
    async fn remove_organization_member(
        &self,
        CurrentUser(user): CurrentUser,
        organization_id: Path<String>,
        user_id: Path<String>,
    ) -> OrganizationMembersResponse {
        let organization_id = match self.owned_organization(&user, &organization_id.0).await {
            Ok(organization_id) => organization_id,
            Err(response) => return response,
        };
        let member_id = match user_id.0.parse::<Id<User>>() {
            Ok(member_id) => member_id,
            Err(_) => return OrganizationMembersResponse::Missing(Json(missing_user())),
        };
        match OrganizationMember::remove(&self.state.database, organization_id, member_id).await {
            Ok(OrganizationMemberChange::Removed) => self.members(organization_id).await,
            Ok(OrganizationMemberChange::Missing) => {
                OrganizationMembersResponse::Missing(Json(missing_user()))
            }
            Ok(OrganizationMemberChange::FinalOwner) => {
                OrganizationMembersResponse::Invalid(Json(Error {
                    message: "an organization must keep an owner".to_owned(),
                }))
            }
            Ok(OrganizationMemberChange::Updated) => {
                OrganizationMembersResponse::Failed(Json(Error {
                    message: "organization membership state was invalid".to_owned(),
                }))
            }
            Err(error) => OrganizationMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/orgs/:organization_id", method = "delete")]
    async fn delete_organization(
        &self,
        CurrentUser(user): CurrentUser,
        organization_id: Path<String>,
    ) -> DeleteOrganizationResponse {
        let organization_id = match organization_id.0.parse::<Id<Organization>>() {
            Ok(organization_id) => organization_id,
            Err(error) => {
                return DeleteOrganizationResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let organization = match organization_access(
            &self.state,
            &user,
            organization_id,
            OrganizationPermission::Owner,
        )
        .await
        {
            OrganizationAccess::Allowed { organization, .. } => organization,
            OrganizationAccess::Forbidden => {
                return DeleteOrganizationResponse::Forbidden(Json(forbidden()));
            }
            OrganizationAccess::Missing => {
                return DeleteOrganizationResponse::Missing(Json(missing_organization()));
            }
            OrganizationAccess::Failed(message) => {
                return DeleteOrganizationResponse::Failed(Json(Error { message }));
            }
        };
        match organization.delete(&self.state.database).await {
            Ok(OrganizationDeletion::Deleted) => DeleteOrganizationResponse::Deleted,
            Ok(OrganizationDeletion::HoldsProjects(held)) => {
                DeleteOrganizationResponse::Invalid(Json(Error {
                    message: format!(
                        "the organization still holds {held} {}, so empty it first",
                        if held == 1 { "project" } else { "projects" }
                    ),
                }))
            }
            Err(error) => DeleteOrganizationResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

impl OrganizationApi {
    /// The caller's own view of one organization. Read after a write so the reader is
    /// answered with the project count and the grant they hold, not just what they sent.
    async fn one_organization(
        &self,
        user: &User,
        organization_id: Id<Organization>,
    ) -> OrganizationResponse {
        match Organization::summaries_for(&self.state.database, user).await {
            Ok(summaries) => summaries
                .into_iter()
                .find(|summary| summary.organization.id == organization_id)
                .map_or_else(
                    || OrganizationResponse::Missing(Json(missing_organization())),
                    |summary| OrganizationResponse::Found(Json(organization_output(summary))),
                ),
            Err(error) => OrganizationResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    async fn owned_organization(
        &self,
        user: &User,
        organization_id: &str,
    ) -> Result<Id<Organization>, OrganizationMembersResponse> {
        let organization_id = organization_id
            .parse::<Id<Organization>>()
            .map_err(|error| {
                OrganizationMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }))
            })?;
        match organization_access(
            &self.state,
            user,
            organization_id,
            OrganizationPermission::Owner,
        )
        .await
        {
            OrganizationAccess::Allowed { .. } => Ok(organization_id),
            OrganizationAccess::Forbidden => {
                Err(OrganizationMembersResponse::Forbidden(Json(forbidden())))
            }
            OrganizationAccess::Missing => Err(OrganizationMembersResponse::Missing(Json(
                missing_organization(),
            ))),
            OrganizationAccess::Failed(message) => {
                Err(OrganizationMembersResponse::Failed(Json(Error { message })))
            }
        }
    }

    async fn members(&self, organization_id: Id<Organization>) -> OrganizationMembersResponse {
        match OrganizationMember::list(&self.state.database, organization_id).await {
            Ok(members) => OrganizationMembersResponse::Found(Json(OrganizationMembersOutput {
                members: members
                    .into_iter()
                    .map(organization_member_output)
                    .collect(),
            })),
            Err(error) => OrganizationMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
pub enum OrganizationRoleOutput {
    Viewer,
    Operator,
    Owner,
}

impl OrganizationRoleOutput {
    pub fn from_role(role: OrganizationRole) -> Self {
        match role {
            OrganizationRole::Viewer => Self::Viewer,
            OrganizationRole::Operator => Self::Operator,
            OrganizationRole::Owner => Self::Owner,
        }
    }

    fn into_role(self) -> OrganizationRole {
        match self {
            Self::Viewer => OrganizationRole::Viewer,
            Self::Operator => OrganizationRole::Operator,
            Self::Owner => OrganizationRole::Owner,
        }
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct OrganizationOutput {
    organization_id: String,
    name: String,
    description: Option<String>,
    viewer_role: OrganizationRoleOutput,
    project_count: u64,
}

#[derive(Debug, Object)]
struct OrganizationsOutput {
    organizations: Vec<OrganizationOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct CreateOrganization {
    name: String,
    description: Option<String>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct OrganizationMemberOutput {
    user_id: String,
    display_name: String,
    role: OrganizationRoleOutput,
}

#[derive(Debug, Object)]
struct OrganizationMembersOutput {
    members: Vec<OrganizationMemberOutput>,
}

#[derive(Debug, Object)]
struct SetOrganizationMember {
    role: OrganizationRoleOutput,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ListOrganizationsResponse {
    #[oai(status = 200)]
    Found(Json<OrganizationsOutput>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum CreateOrganizationResponse {
    #[oai(status = 201)]
    Created(Json<OrganizationOutput>),
    #[oai(status = 400)]
    Invalid(Json<Error>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 403)]
    Forbidden(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum DeleteOrganizationResponse {
    #[oai(status = 204)]
    Deleted,
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

#[allow(dead_code)]
#[derive(ApiResponse)]
enum OrganizationResponse {
    #[oai(status = 200)]
    Found(Json<OrganizationOutput>),
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

#[allow(dead_code)]
#[derive(ApiResponse)]
enum OrganizationMembersResponse {
    #[oai(status = 200)]
    Found(Json<OrganizationMembersOutput>),
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

fn organization_output(summary: OrganizationSummary) -> OrganizationOutput {
    OrganizationOutput {
        organization_id: summary.organization.id.encode(),
        name: summary.organization.name,
        description: summary.organization.description,
        viewer_role: OrganizationRoleOutput::from_role(summary.viewer_role),
        project_count: summary.project_count,
    }
}

fn organization_member_output(member: OrganizationMember) -> OrganizationMemberOutput {
    OrganizationMemberOutput {
        user_id: member.user_id.encode(),
        display_name: member.display_name,
        role: OrganizationRoleOutput::from_role(member.role),
    }
}
