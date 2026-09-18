use std::sync::Arc;

use jiff::Timestamp;
use poem::http::StatusCode;
use poem::web::cookie::{Cookie, SameSite};
use poem::web::{Data, Query};
use poem::{
    Endpoint, EndpointExt, Error, FromRequest, IntoResponse, Middleware, Request, RequestBody,
    Response, Route,
};
use rand::distr::Alphanumeric;
use rand::{Rng, rng};
use serde::Deserialize;
use url::Url;

use crate::store::{Store, StoreError};
use crate::user::User;

const GITHUB_ISSUER: &str = "https://github.com";
const SESSION_SECONDS: u64 = 60 * 60 * 24 * 14;
const ATTEMPT_SECONDS: u64 = 600;

const SESSION_COOKIE: &str = "__Host-error-menu-session";
const STATE_COOKIE: &str = "__Host-error-menu-oauth-state";

fn log_store_error(operation: &'static str, error: &StoreError) {
    match error.database_code() {
        Some(database_code) => tracing::error!(
            operation,
            error_kind = error.kind(),
            database_code,
            "authentication storage operation failed"
        ),
        None => tracing::error!(
            operation,
            error_kind = error.kind(),
            "authentication storage operation failed"
        ),
    }
}

fn cookie_value(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .get_all("cookie")
        .iter()
        .find_map(|value| {
            value.to_str().ok().and_then(|header| {
                header.split(';').map(str::trim).find_map(|item| {
                    Cookie::parse(item)
                        .ok()
                        .filter(|cookie| cookie.name() == name)
                        .map(|cookie| cookie.value_str().to_owned())
                })
            })
        })
}

fn clear_cookie(mut response: Response, name: &str) -> Response {
    let mut cookie = Cookie::named(name);
    cookie.set_path("/");
    cookie.set_secure(true);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.make_removal();
    response.headers_mut().append(
        "set-cookie",
        cookie.to_string().parse().expect("valid cookie"),
    );
    response
}

fn clear_state_cookie(response: Response) -> Response {
    clear_cookie(response, STATE_COOKIE)
}

fn unauthenticated() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .content_type("application/json; charset=utf-8")
        .body(r#"{"message":"authentication is required"}"#)
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

pub struct RequireSession {
    store: Arc<Store>,
}

impl RequireSession {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

pub struct RequireSessionEndpoint<E> {
    endpoint: E,
    store: Arc<Store>,
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
            store: Arc::clone(&self.store),
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
        let Some(token) = cookie_value(&request, SESSION_COOKIE) else {
            return Ok(unauthenticated());
        };
        let user = match self
            .store
            .user_for_session(
                &blake3::hash(token.as_bytes()).to_hex().to_string(),
                Timestamp::now(),
            )
            .await
        {
            Ok(user) => user,
            Err(error) => {
                log_store_error("lookup_session", &error);
                None
            }
        };
        let Some(user) = user else {
            return Ok(unauthenticated());
        };
        request.extensions_mut().insert(CurrentUser(user));
        Ok(self.endpoint.call(request).await?.into_response())
    }
}

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
    store: Arc<Store>,
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
    pub fn from_environment(store: Arc<Store>) -> Result<Arc<Self>, GithubAuthConfigError> {
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
            store,
            client_id,
            client_secret,
            callback_url,
            allow_registration,
            client,
        }))
    }

    fn random_token() -> String {
        rng()
            .sample_iter(Alphanumeric)
            .take(64)
            .map(char::from)
            .collect()
    }
}

#[poem::handler]
async fn login(Data(auth): Data<&Arc<GithubAuth>>) -> Response {
    let state = GithubAuth::random_token();
    let expires_at = Timestamp::now() + std::time::Duration::from_secs(ATTEMPT_SECONDS);
    if let Err(error) = auth
        .store
        .create_auth_attempt(
            &blake3::hash(state.as_bytes()).to_hex().to_string(),
            expires_at,
        )
        .await
    {
        log_store_error("create_auth_attempt", &error);
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .finish();
    }
    let mut url = Url::parse("https://github.com/login/oauth/authorize").expect("valid GitHub URL");
    url.query_pairs_mut()
        .append_pair("client_id", &auth.client_id)
        .append_pair("redirect_uri", &auth.callback_url)
        .append_pair("scope", "read:user")
        .append_pair("state", &state);
    let mut state_cookie = Cookie::new_with_str(STATE_COOKIE, &state);
    state_cookie.set_http_only(true);
    state_cookie.set_secure(true);
    state_cookie.set_same_site(SameSite::Lax);
    state_cookie.set_path("/");
    Response::builder()
        .status(StatusCode::FOUND)
        .header("location", url.as_str())
        .header("set-cookie", state_cookie.to_string())
        .finish()
}

#[poem::handler]
async fn callback(
    Data(auth): Data<&Arc<GithubAuth>>,
    Query(callback): Query<Callback>,
    request: &poem::Request,
) -> Response {
    let state_matches = cookie_value(request, STATE_COOKIE).is_some_and(|state| {
        blake3::hash(state.as_bytes()) == blake3::hash(callback.state.as_bytes())
    });
    if !state_matches {
        return clear_state_cookie(Response::builder().status(StatusCode::BAD_REQUEST).finish());
    }
    let state_hash = blake3::hash(callback.state.as_bytes()).to_hex().to_string();
    let valid = match auth
        .store
        .consume_auth_attempt(&state_hash, Timestamp::now())
        .await
    {
        Ok(valid) => valid,
        Err(error) => {
            log_store_error("consume_auth_attempt", &error);
            false
        }
    };
    if !valid {
        return clear_state_cookie(Response::builder().status(StatusCode::BAD_REQUEST).finish());
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
            Err(_) => {
                tracing::error!(
                    operation = "decode_github_token_response",
                    "GitHub OAuth operation failed"
                );
                return clear_state_cookie(
                    Response::builder().status(StatusCode::BAD_GATEWAY).finish(),
                );
            }
        },
        Err(_) => {
            tracing::error!(
                operation = "exchange_github_code",
                "GitHub OAuth operation failed"
            );
            return clear_state_cookie(
                Response::builder().status(StatusCode::BAD_GATEWAY).finish(),
            );
        }
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
            Err(_) => {
                tracing::error!(
                    operation = "decode_github_user_response",
                    "GitHub OAuth operation failed"
                );
                return clear_state_cookie(
                    Response::builder().status(StatusCode::BAD_GATEWAY).finish(),
                );
            }
        },
        Err(_) => {
            tracing::error!(
                operation = "fetch_github_user",
                "GitHub OAuth operation failed"
            );
            return clear_state_cookie(
                Response::builder().status(StatusCode::BAD_GATEWAY).finish(),
            );
        }
    };
    let Some(user) = (match auth
        .store
        .register_user(
            GITHUB_ISSUER,
            &github_user.id.to_string(),
            &github_user.login,
            auth.allow_registration,
        )
        .await
    {
        Ok(user) => user,
        Err(error) => {
            log_store_error("register_user", &error);
            return clear_state_cookie(
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .finish(),
            );
        }
    }) else {
        return clear_state_cookie(Response::builder().status(StatusCode::FORBIDDEN).finish());
    };
    let session = GithubAuth::random_token();
    let expires_at = Timestamp::now() + std::time::Duration::from_secs(SESSION_SECONDS);
    if let Err(error) = auth
        .store
        .create_session(
            user.id,
            &blake3::hash(session.as_bytes()).to_hex().to_string(),
            expires_at,
        )
        .await
    {
        log_store_error("create_session", &error);
        return clear_state_cookie(
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish(),
        );
    }
    let mut session_cookie = Cookie::new_with_str(SESSION_COOKIE, session);
    session_cookie.set_http_only(true);
    session_cookie.set_secure(true);
    session_cookie.set_same_site(SameSite::Lax);
    session_cookie.set_path("/");
    clear_state_cookie(
        Response::builder()
            .status(StatusCode::FOUND)
            .header("location", "/")
            .header("set-cookie", session_cookie.to_string())
            .finish(),
    )
}

#[poem::handler]
async fn logout(Data(auth): Data<&Arc<GithubAuth>>, request: &poem::Request) -> Response {
    if let Some(token) = cookie_value(request, SESSION_COOKIE) {
        if let Err(error) = auth
            .store
            .delete_session(&blake3::hash(token.as_bytes()).to_hex().to_string())
            .await
        {
            log_store_error("delete_session", &error);
        }
    }

    clear_cookie(
        Response::builder().status(StatusCode::NO_CONTENT).finish(),
        SESSION_COOKIE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::test::TestClient;

    #[poem::handler]
    async fn protected(CurrentUser(user): CurrentUser) -> String {
        user.display_name
    }

    fn github_auth(store: Arc<Store>) -> Arc<GithubAuth> {
        Arc::new(GithubAuth {
            store,
            client_id: "client".to_owned(),
            client_secret: "secret".to_owned(),
            callback_url: "https://error.menu/auth/github/callback".to_owned(),
            allow_registration: true,
            client: reqwest::Client::new(),
        })
    }

    #[tokio::test]
    async fn login_persists_state_before_redirecting() {
        let store = Arc::new(Store::open("sqlite::memory:", 0).await.expect("opens"));
        let client = TestClient::new(routes(github_auth(store)));

        client
            .get("/github/login")
            .send()
            .await
            .assert_status(StatusCode::FOUND);
    }

    #[tokio::test]
    async fn middleware_adds_verified_session_user_to_request() {
        let store = Arc::new(Store::open("sqlite::memory:", 0).await.expect("opens"));
        let user = store
            .register_user(GITHUB_ISSUER, "1", "luc", true)
            .await
            .expect("registers")
            .expect("allows first user");
        let token = "test-session";
        store
            .create_session(
                user.id,
                &blake3::hash(token.as_bytes()).to_hex().to_string(),
                Timestamp::now() + std::time::Duration::from_secs(60),
            )
            .await
            .expect("stores session");
        let client = TestClient::new(protected.with(RequireSession::new(Arc::clone(&store))));

        client
            .get("/")
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        client
            .get("/")
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .send()
            .await
            .assert_text("luc")
            .await;

        let guest = store
            .register_user(GITHUB_ISSUER, "2", "guest", true)
            .await
            .expect("registers")
            .expect("allows guests");
        let guest_token = "guest-session";
        store
            .create_session(
                guest.id,
                &blake3::hash(guest_token.as_bytes()).to_hex().to_string(),
                Timestamp::now() + std::time::Duration::from_secs(60),
            )
            .await
            .expect("stores guest session");
        client
            .get("/")
            .header("cookie", format!("{SESSION_COOKIE}={guest_token}"))
            .send()
            .await
            .assert_text("guest")
            .await;
    }
    #[tokio::test]
    async fn logout_clears_the_cookie_and_revokes_the_session() {
        let store = Arc::new(Store::open("sqlite::memory:", 0).await.expect("opens"));
        let user = store
            .register_user(GITHUB_ISSUER, "1", "luc", true)
            .await
            .expect("registers")
            .expect("allows first user");
        let token = "test-session";
        let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        store
            .create_session(
                user.id,
                &token_hash,
                Timestamp::now() + std::time::Duration::from_secs(60),
            )
            .await
            .expect("stores session");
        let auth = github_auth(Arc::clone(&store));
        let client = TestClient::new(routes(auth));

        let response = client
            .post("/logout")
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .send()
            .await;
        response.assert_status(StatusCode::NO_CONTENT);
        assert!(
            store
                .user_for_session(&token_hash, Timestamp::now())
                .await
                .expect("reads session")
                .is_none()
        );
    }
}

pub fn routes(auth: Arc<GithubAuth>) -> Route {
    Route::new()
        .at("/github/login", poem::get(login).data(Arc::clone(&auth)))
        .at(
            "/github/callback",
            poem::get(callback).data(Arc::clone(&auth)),
        )
        .at("/logout", poem::post(logout).data(auth))
}
