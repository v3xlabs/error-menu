use std::sync::Arc;

use jiff::Timestamp;
use poem::http::StatusCode;
use poem::{
    Endpoint, Error, FromRequest, IntoResponse, Middleware, Request, RequestBody, Response,
};

use crate::app::AppState;
use crate::prelude::*;

use super::{SESSION_COOKIE, ScopedCookie, log_database_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    Session,
    Bearer,
}

#[derive(Clone, Debug)]
pub struct CurrentUser(pub User);

impl<'a> FromRequest<'a> for CurrentUser {
    async fn from_request(request: &'a Request, _: &mut RequestBody) -> poem::Result<Self> {
        request.extensions().get::<Self>().cloned().ok_or_else(|| {
            Error::from_string("missing authenticated user", StatusCode::UNAUTHORIZED)
        })
    }
}

#[derive(Clone, Debug)]
pub struct CurrentCredential(pub CredentialKind);

impl<'a> FromRequest<'a> for CurrentCredential {
    async fn from_request(request: &'a Request, _: &mut RequestBody) -> poem::Result<Self> {
        request.extensions().get::<Self>().cloned().ok_or_else(|| {
            Error::from_string("missing authenticated credential", StatusCode::UNAUTHORIZED)
        })
    }
}

#[derive(Clone, Debug)]
pub struct SessionUser(pub User);

impl<'a> FromRequest<'a> for SessionUser {
    async fn from_request(request: &'a Request, _: &mut RequestBody) -> poem::Result<Self> {
        let user = request.extensions().get::<CurrentUser>();
        let credential = request.extensions().get::<CurrentCredential>();
        match (user, credential) {
            (Some(user), Some(credential)) if credential.0 == CredentialKind::Session => {
                Ok(Self(user.0.clone()))
            }
            _ => Err(Error::from_string(
                "missing session user",
                StatusCode::FORBIDDEN,
            )),
        }
    }
}

pub struct RequireSession {
    state: Arc<AppState>,
}

impl RequireSession {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

pub struct RequireSessionEndpoint<E> {
    endpoint: E,
    state: Arc<AppState>,
}

impl<E> Middleware<E> for RequireSession
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    type Output = RequireSessionEndpoint<E>;

    fn transform(&self, endpoint: E) -> Self::Output {
        RequireSessionEndpoint {
            endpoint,
            state: Arc::clone(&self.state),
        }
    }
}

impl<E> Endpoint for RequireSessionEndpoint<E>
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, mut request: Request) -> poem::Result<Self::Output> {
        if matches!(request.uri().path(), "/health" | "/api/health") {
            return Ok(self.endpoint.call(request).await?.into_response());
        }
        let bearer_hash = match bearer_token(&request) {
            Ok(Some(token)) => {
                if !matches!(
                    request.method(),
                    &poem::http::Method::GET | &poem::http::Method::HEAD
                ) && !(request.method() == poem::http::Method::POST
                    && request.uri().path() == "/mcp")
                {
                    return Ok(bearer_write_forbidden());
                }
                Some(blake3::hash(token.as_bytes()).to_hex().to_string())
            }
            Ok(None) => None,
            Err(()) => return Ok(unauthenticated()),
        };
        let authenticated = match bearer_hash {
            Some(token_hash) => match crate::user::token::ApiToken::user_for(
                &self.state.database,
                &token_hash,
                Timestamp::now(),
            )
            .await
            {
                Ok(user) => user.map(|user| (user, CredentialKind::Bearer)),
                Err(error) => {
                    log_database_error("lookup_api_token", &error);
                    return Ok(credential_unavailable());
                }
            },
            None => {
                let Some(token) = ScopedCookie(SESSION_COOKIE).read(&request) else {
                    return Ok(unauthenticated());
                };
                let hash = blake3::hash(token.as_bytes()).to_hex().to_string();
                match crate::user::session::user_for(&self.state.database, &hash, Timestamp::now())
                    .await
                {
                    Ok(user) => user.map(|user| (user, CredentialKind::Session)),
                    Err(error) => {
                        log_database_error("lookup_session", &error);
                        return Ok(credential_unavailable());
                    }
                }
            }
        };
        let Some((user, credential)) = authenticated else {
            return Ok(unauthenticated());
        };
        request
            .extensions_mut()
            .insert(CurrentCredential(credential));
        request.extensions_mut().insert(CurrentUser(user));
        Ok(self.endpoint.call(request).await?.into_response())
    }
}

fn bearer_token(request: &Request) -> Result<Option<String>, ()> {
    let Some(value) = request.headers().get("authorization") else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| ())?;
    let mut parts = value.split_ascii_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(scheme), Some(token), None) if scheme.eq_ignore_ascii_case("bearer") => {
            Ok(Some(token.to_owned()))
        }
        _ => Err(()),
    }
}

fn unauthenticated() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .content_type("application/json; charset=utf-8")
        .body(r#"{"message":"authentication is required"}"#)
}

/// A credential that cannot be read is not a credential that was refused. Reporting the
/// storage failure keeps a database stall out of the sign-in path a reader would retry.
fn credential_unavailable() -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .content_type("application/json; charset=utf-8")
        .body(r#"{"message":"the credential store is unavailable"}"#)
}

fn bearer_write_forbidden() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .content_type("application/json; charset=utf-8")
        .body(r#"{"message":"bearer tokens are read-only"}"#)
}
