use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::app::AppState;
use crate::http::api::{Error, forbidden, missing_user, user_output};
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::user::UserRoleChange;

pub struct UserApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl UserApi {
    #[oai(path = "/user", method = "get")]
    async fn current_user(&self, CurrentUser(user): CurrentUser) -> UserResponse {
        UserResponse::Found(Json(user_output(user)))
    }

    #[oai(path = "/users", method = "get")]
    async fn list_users(&self, CurrentUser(user): CurrentUser) -> ListUsersResponse {
        if user.role == UserRole::Guest {
            return ListUsersResponse::Forbidden(Json(forbidden()));
        }

        match User::list(&self.state.database).await {
            Ok(users) => ListUsersResponse::Found(Json(UsersOutput {
                users: users.into_iter().map(user_output).collect(),
            })),
            Err(error) => ListUsersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/users/:user_id", method = "get")]
    async fn user(&self, CurrentUser(user): CurrentUser, user_id: Path<String>) -> UserResponse {
        if user.role == UserRole::Guest {
            return UserResponse::Forbidden(Json(forbidden()));
        }
        let user_id = match user_id.0.parse::<Id<User>>() {
            Ok(user_id) => user_id,
            Err(_) => return UserResponse::Missing(Json(missing_user())),
        };

        match User::load(&self.state.database, user_id).await {
            Ok(Some(user)) => UserResponse::Found(Json(user_output(user))),
            Ok(None) => UserResponse::Missing(Json(missing_user())),
            Err(error) => UserResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/users/:user_id/role", method = "put")]
    async fn set_user_role(
        &self,
        CurrentUser(user): CurrentUser,
        user_id: Path<String>,
        input: Json<SetUserRole>,
    ) -> UserResponse {
        if user.role != UserRole::Admin {
            return UserResponse::Forbidden(Json(forbidden()));
        }
        let user_id = match user_id.0.parse::<Id<User>>() {
            Ok(user_id) => user_id,
            Err(_) => return UserResponse::Missing(Json(missing_user())),
        };

        match User::set_role(&self.state.database, user_id, input.0.role.into_role()).await {
            Ok(UserRoleChange::Updated) => match User::load(&self.state.database, user_id).await {
                Ok(Some(user)) => UserResponse::Found(Json(user_output(user))),
                Ok(None) => UserResponse::Missing(Json(missing_user())),
                Err(error) => UserResponse::Failed(Json(Error {
                    message: error.to_string(),
                })),
            },
            Ok(UserRoleChange::Missing) => UserResponse::Missing(Json(missing_user())),
            Ok(UserRoleChange::FinalAdmin) => UserResponse::Invalid(Json(Error {
                message: "the final administrator cannot be demoted".to_owned(),
            })),
            Err(error) => UserResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
pub enum UserRoleOutput {
    Guest,
    Member,
    Admin,
}

impl UserRoleOutput {
    pub fn from_role(role: UserRole) -> Self {
        match role {
            UserRole::Guest => Self::Guest,
            UserRole::Member => Self::Member,
            UserRole::Admin => Self::Admin,
        }
    }

    fn into_role(self) -> UserRole {
        match self {
            Self::Guest => UserRole::Guest,
            Self::Member => UserRole::Member,
            Self::Admin => UserRole::Admin,
        }
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct UserOutput {
    pub user_id: String,
    pub display_name: String,
    pub role: UserRoleOutput,
}

#[derive(Debug, Object)]
struct UsersOutput {
    users: Vec<UserOutput>,
}

#[derive(Debug, Object)]
struct SetUserRole {
    role: UserRoleOutput,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum UserResponse {
    #[oai(status = 200)]
    Found(Json<UserOutput>),
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
enum ListUsersResponse {
    #[oai(status = 200)]
    Found(Json<UsersOutput>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 403)]
    Forbidden(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}
