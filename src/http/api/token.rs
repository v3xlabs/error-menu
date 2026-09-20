use std::sync::Arc;

use jiff::Timestamp;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Object, OpenApi};

use crate::app::AppState;
use crate::http::api::{Error, api_token_error};
use crate::http::auth::{self, SessionUser};
use crate::user::token::ApiToken;

pub struct TokenApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl TokenApi {
    #[oai(path = "/tokens", method = "get")]
    async fn list_api_tokens(&self, SessionUser(user): SessionUser) -> ApiTokensResponse {
        match ApiToken::list(&self.state.database, user.id, Timestamp::now()).await {
            Ok(tokens) => ApiTokensResponse::Found(Json(ApiTokensOutput {
                tokens: tokens
                    .into_iter()
                    .map(|token| ApiTokenOutput {
                        name: token.name,
                        created_at: token.created_at.to_string(),
                        expires_at: token.expires_at.map(|expires_at| expires_at.to_string()),
                    })
                    .collect(),
            })),
            Err(error) => {
                ApiTokensResponse::Failed(Json(api_token_error("list_api_tokens", error)))
            }
        }
    }

    #[oai(path = "/tokens", method = "post")]
    async fn create_api_token(
        &self,
        SessionUser(user): SessionUser,
        input: Json<CreateApiToken>,
    ) -> ApiTokensResponse {
        let input = input.0;
        let name = input.name.trim();
        if name.is_empty() || name.len() > 128 || matches!(name, "." | "..") {
            return ApiTokensResponse::Invalid(Json(Error {
                message: "token name must contain 1 to 128 bytes and cannot be . or ..".to_owned(),
            }));
        }
        let now = Timestamp::now();
        let expires_at = match input
            .expires_at
            .as_deref()
            .map(str::parse::<Timestamp>)
            .transpose()
        {
            Ok(expires_at) => expires_at,
            Err(_) => {
                return ApiTokensResponse::Invalid(Json(Error {
                    message: "token expiry must be an RFC 3339 timestamp".to_owned(),
                }));
            }
        };
        if expires_at.is_some_and(|expires_at| expires_at.as_millisecond() <= now.as_millisecond())
        {
            return ApiTokensResponse::Invalid(Json(Error {
                message: "token expiry must be in the future".to_owned(),
            }));
        }
        let token = format!("em_pat_{}", auth::random_token());
        let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        match ApiToken::create(&self.state.database, user.id, &token_hash, name, expires_at).await {
            Ok(stored) => ApiTokensResponse::Created(Json(CreatedApiTokenOutput {
                name: stored.name,
                token,
                created_at: stored.created_at.to_string(),
                expires_at: stored.expires_at.map(|expires_at| expires_at.to_string()),
            })),
            Err(error) => {
                ApiTokensResponse::Failed(Json(api_token_error("create_api_token", error)))
            }
        }
    }

    #[oai(path = "/tokens/:name", method = "delete")]
    async fn revoke_api_token(
        &self,
        SessionUser(user): SessionUser,
        name: Path<String>,
    ) -> RevokeApiTokenResponse {
        match ApiToken::revoke(&self.state.database, user.id, &name.0).await {
            Ok(true) => RevokeApiTokenResponse::Revoked,
            Ok(false) => RevokeApiTokenResponse::Missing(Json(Error {
                message: "API token not found".to_owned(),
            })),
            Err(error) => {
                RevokeApiTokenResponse::Failed(Json(api_token_error("revoke_api_token", error)))
            }
        }
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ApiTokenOutput {
    name: String,
    created_at: String,
    expires_at: Option<String>,
}

#[derive(Debug, Object)]
struct ApiTokensOutput {
    tokens: Vec<ApiTokenOutput>,
}

#[derive(Debug, Object)]
struct CreateApiToken {
    name: String,
    expires_at: Option<String>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct CreatedApiTokenOutput {
    name: String,
    token: String,
    created_at: String,
    expires_at: Option<String>,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ApiTokensResponse {
    #[oai(status = 200)]
    Found(Json<ApiTokensOutput>),
    #[oai(status = 201)]
    Created(Json<CreatedApiTokenOutput>),
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
enum RevokeApiTokenResponse {
    #[oai(status = 204)]
    Revoked,
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 403)]
    Forbidden(Json<Error>),
    #[oai(status = 404)]
    Missing(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}
