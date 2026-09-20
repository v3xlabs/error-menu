use std::sync::Arc;

use jiff::Timestamp;
use poem::http::StatusCode;
use poem::web::{Data, Query};
use poem::{EndpointExt, Response, Route};
use serde::Deserialize;
use url::Url;

use crate::app::AppState;
use crate::prelude::*;

use super::auth::{
    SESSION_COOKIE, SESSION_SECONDS, ScopedCookie, log_database_error, random_token,
};

const GITHUB_ISSUER: &str = "https://github.com";
const ATTEMPT_SECONDS: u64 = 600;
const STATE_COOKIE: &str = "__Host-error-menu-oauth-state";

#[derive(Debug, thiserror::Error)]
pub enum GithubAuthConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("PUBLIC_ORIGIN must be an absolute HTTPS origin without a path")]
    PublicOrigin,
    #[error("could not initialise the GitHub OAuth HTTP client")]
    HttpClient,
}

pub struct GithubAuth {
    state: Arc<AppState>,
    client_id: String,
    client_secret: String,
    callback_url: String,
    allow_registration: bool,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct Callback {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
}

impl GithubAuth {
    #[cfg(debug_assertions)]
    fn development(state: Arc<AppState>) -> Result<Arc<Self>, GithubAuthConfigError> {
        Ok(Arc::new(Self {
            state,
            client_id: "development".to_owned(),
            client_secret: "development".to_owned(),
            callback_url: "https://development.invalid/auth/github/callback".to_owned(),
            allow_registration: true,
            client: reqwest::Client::new(),
        }))
    }

    pub fn from_environment(state: Arc<AppState>) -> Result<Arc<Self>, GithubAuthConfigError> {
        #[cfg(debug_assertions)]
        if matches!(
            std::env::var("ERROR_MENU_DEV_LOGIN").as_deref(),
            Ok("1" | "true" | "TRUE")
        ) {
            return Self::development(state);
        }
        let client_id = std::env::var("GITHUB_CLIENT_ID")
            .map_err(|_| GithubAuthConfigError::Missing("GITHUB_CLIENT_ID"))?;
        let client_secret = std::env::var("GITHUB_CLIENT_SECRET")
            .map_err(|_| GithubAuthConfigError::Missing("GITHUB_CLIENT_SECRET"))?;
        let origin = std::env::var("PUBLIC_ORIGIN")
            .map_err(|_| GithubAuthConfigError::Missing("PUBLIC_ORIGIN"))?;
        let origin = Url::parse(&origin).map_err(|_| GithubAuthConfigError::PublicOrigin)?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(GithubAuthConfigError::PublicOrigin);
        }
        let callback_url = origin
            .join("auth/github/callback")
            .map_err(|_| GithubAuthConfigError::PublicOrigin)?
            .to_string();
        let allow_registration = matches!(
            std::env::var("ALLOW_REGISTRATION").as_deref(),
            Ok("1" | "true" | "TRUE")
        );
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| GithubAuthConfigError::HttpClient)?;
        Ok(Arc::new(Self {
            state,
            client_id,
            client_secret,
            callback_url,
            allow_registration,
            client,
        }))
    }
}

pub fn routes(auth: Arc<GithubAuth>) -> Route {
    let routes = Route::new()
        .at("/github/login", poem::get(login).data(Arc::clone(&auth)))
        .at(
            "/github/callback",
            poem::get(callback).data(Arc::clone(&auth)),
        )
        .at("/logout", poem::post(logout).data(Arc::clone(&auth)));
    #[cfg(debug_assertions)]
    let routes = routes.at("/dev/login", poem::get(development_login).data(auth));
    routes
}

fn clear_state_cookie(response: Response) -> Response {
    ScopedCookie(STATE_COOKIE).clear(response)
}

fn status(code: StatusCode) -> Response {
    Response::builder().status(code).finish()
}

fn github_failed(operation: &'static str) -> Response {
    tracing::error!(operation, "GitHub OAuth operation failed");

    clear_state_cookie(status(StatusCode::BAD_GATEWAY))
}

fn signed_in(session: String) -> Response {
    clear_state_cookie(
        Response::builder()
            .status(StatusCode::FOUND)
            .header("location", "/")
            .header(
                "set-cookie",
                ScopedCookie(SESSION_COOKIE).set(&session).to_string(),
            )
            .finish(),
    )
}

#[poem::handler]
async fn login(Data(auth): Data<&Arc<GithubAuth>>) -> Response {
    let state = random_token();
    let expires_at = Timestamp::now() + std::time::Duration::from_secs(ATTEMPT_SECONDS);
    if let Err(error) = crate::user::attempt::create(
        &auth.state.database,
        &blake3::hash(state.as_bytes()).to_hex(),
        expires_at,
    )
    .await
    {
        log_database_error("create_auth_attempt", &error);

        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut url = Url::parse("https://github.com/login/oauth/authorize").expect("valid GitHub URL");
    url.query_pairs_mut()
        .append_pair("client_id", &auth.client_id)
        .append_pair("redirect_uri", &auth.callback_url)
        .append_pair("scope", "read:user")
        .append_pair("state", &state);

    Response::builder()
        .status(StatusCode::FOUND)
        .header("location", url.as_str())
        .header(
            "set-cookie",
            ScopedCookie(STATE_COOKIE).set(&state).to_string(),
        )
        .finish()
}

#[poem::handler]
async fn callback(
    Data(auth): Data<&Arc<GithubAuth>>,
    Query(callback): Query<Callback>,
    request: &poem::Request,
) -> Response {
    let state_matches = ScopedCookie(STATE_COOKIE)
        .read(request)
        .is_some_and(|state| {
            blake3::hash(state.as_bytes()) == blake3::hash(callback.state.as_bytes())
        });
    if !state_matches {
        return clear_state_cookie(status(StatusCode::BAD_REQUEST));
    }
    let state_hash = blake3::hash(callback.state.as_bytes()).to_hex().to_string();
    let valid =
        match crate::user::attempt::consume(&auth.state.database, &state_hash, Timestamp::now())
            .await
        {
            Ok(valid) => valid,
            Err(error) => {
                log_database_error("consume_auth_attempt", &error);
                false
            }
        };
    if !valid {
        return clear_state_cookie(status(StatusCode::BAD_REQUEST));
    }
    let token = match auth
        .client
        .post("https://github.com/login/oauth/access_token")
        .header("accept", "application/json")
        .form(&[
            ("client_id", auth.client_id.as_str()),
            ("client_secret", auth.client_secret.as_str()),
            ("code", callback.code.as_str()),
            ("redirect_uri", auth.callback_url.as_str()),
        ])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        Ok(response) => match response.json::<TokenResponse>().await {
            Ok(token) => token,
            Err(_) => return github_failed("decode_github_token_response"),
        },
        Err(_) => return github_failed("exchange_github_code"),
    };
    let github_user = match auth
        .client
        .get("https://api.github.com/user")
        .bearer_auth(token.access_token)
        .header("user-agent", "error.menu")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        Ok(response) => match response.json::<GithubUser>().await {
            Ok(user) => user,
            Err(_) => return github_failed("decode_github_user_response"),
        },
        Err(_) => return github_failed("fetch_github_user"),
    };
    let Some(user) = (match User::register(
        &auth.state.database,
        GITHUB_ISSUER,
        &github_user.id.to_string(),
        &github_user.login,
        auth.allow_registration,
    )
    .await
    {
        Ok(user) => user,
        Err(error) => {
            log_database_error("register_user", &error);

            return clear_state_cookie(status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    }) else {
        return clear_state_cookie(status(StatusCode::FORBIDDEN));
    };
    let session = random_token();
    let expires_at = Timestamp::now() + std::time::Duration::from_secs(SESSION_SECONDS);
    if let Err(error) = crate::user::session::create(
        &auth.state.database,
        user.id,
        &blake3::hash(session.as_bytes()).to_hex(),
        expires_at,
    )
    .await
    {
        log_database_error("create_session", &error);

        return clear_state_cookie(status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    signed_in(session)
}

#[poem::handler]
async fn logout(Data(auth): Data<&Arc<GithubAuth>>, request: &poem::Request) -> Response {
    if let Some(token) = ScopedCookie(SESSION_COOKIE).read(request)
        && let Err(error) = crate::user::session::delete(
            &auth.state.database,
            &blake3::hash(token.as_bytes()).to_hex(),
        )
        .await
    {
        log_database_error("delete_session", &error);
    }

    ScopedCookie(SESSION_COOKIE).clear(status(StatusCode::NO_CONTENT))
}

#[cfg(debug_assertions)]
#[poem::handler]
async fn development_login(Data(auth): Data<&Arc<GithubAuth>>) -> Response {
    if !matches!(
        std::env::var("ERROR_MENU_DEV_LOGIN").as_deref(),
        Ok("1" | "true" | "TRUE")
    ) {
        return status(StatusCode::NOT_FOUND);
    }
    let user = match User::register(
        &auth.state.database,
        "urn:error-menu:development",
        "developer",
        "Developer",
        true,
    )
    .await
    {
        Ok(Some(user)) => user,
        _ => return status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let session = random_token();
    if crate::user::session::create(
        &auth.state.database,
        user.id,
        &blake3::hash(session.as_bytes()).to_hex(),
        Timestamp::now() + std::time::Duration::from_secs(SESSION_SECONDS),
    )
    .await
    .is_err()
    {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }

    signed_in(session)
}
