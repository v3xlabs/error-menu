//! The GitHub App a server registers once. It is the only GitHub integration that may write
//! a check, and it writes as itself rather than as a person. An account opts a repository in
//! by installing it; error.menu learns about the install from a signed webhook.

use std::collections::HashMap;
use std::time::Duration;

use hmac::{Hmac, Mac};
use jiff::Timestamp;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::{Mutex, OnceCell};

use crate::forge::github::API_HOST;
use crate::forge::reader::{ForgeReadError, refusal};

/// An installation token lives an hour. One this close to its end is replaced rather than
/// sent, so a request that starts with it does not arrive after it expired.
const TOKEN_MARGIN: Duration = Duration::from_secs(300);

/// GitHub rejects an app JWT that lives longer than ten minutes, and one issued by a clock
/// slightly ahead of GitHub's. The start is set back a minute for that drift.
const JWT_BACKDATE_SECONDS: i64 = 60;
const JWT_LIFETIME_SECONDS: i64 = 540;

#[derive(Debug, thiserror::Error)]
pub enum GithubAppConfigError {
    #[error("the GitHub App is half configured: {0} is missing")]
    Missing(&'static str),
    #[error("GITHUB_APP_ID is not a number")]
    AppId,
    #[error("reading GITHUB_APP_PRIVATE_KEY_FILE: {0}")]
    KeyFile(#[from] std::io::Error),
    #[error("GITHUB_APP_PRIVATE_KEY_FILE is not an RSA private key in PEM form: {0}")]
    Key(#[from] jsonwebtoken::errors::Error),
    #[error("PUBLIC_ORIGIN must be an https URL")]
    PublicOrigin,
    #[error("building the GitHub client: {0}")]
    Client(#[from] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum GithubAppError {
    #[error("GitHub has no request budget left for the app until {reset}")]
    RateLimited { reset: Timestamp },
    #[error("GitHub refused the app with status {status}")]
    Refused { status: u16 },
    #[error("requesting GitHub: {0}")]
    Request(#[from] reqwest::Error),
    #[error("signing the app token: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
}

pub struct GithubApp {
    id: u64,
    key: EncodingKey,
    webhook_secret: Vec<u8>,
    public_origin: reqwest::Url,
    client: reqwest::Client,
    slug: OnceCell<String>,
    tokens: Mutex<HashMap<(i64, String), InstallationToken>>,
}

struct InstallationToken {
    token: String,
    expires_at: Timestamp,
}

#[derive(Serialize)]
struct Claims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    repositories: [&'a str; 1],
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    expires_at: Timestamp,
}

#[derive(Deserialize)]
struct AppResponse {
    slug: String,
}

impl GithubApp {
    /// No variable set is a server that stays read only. Some set and some missing is a
    /// mistake, and starting anyway would hide it until the first pull request.
    pub fn from_environment() -> Result<Option<Self>, GithubAppConfigError> {
        let variable = |name: &'static str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let (id, key_file, secret) = match (
            variable("GITHUB_APP_ID"),
            variable("GITHUB_APP_PRIVATE_KEY_FILE"),
            variable("GITHUB_WEBHOOK_SECRET"),
        ) {
            (None, None, None) => return Ok(None),
            (Some(id), Some(key_file), Some(secret)) => (id, key_file, secret),
            (None, _, _) => return Err(GithubAppConfigError::Missing("GITHUB_APP_ID")),
            (_, None, _) => {
                return Err(GithubAppConfigError::Missing("GITHUB_APP_PRIVATE_KEY_FILE"));
            }
            (_, _, None) => return Err(GithubAppConfigError::Missing("GITHUB_WEBHOOK_SECRET")),
        };
        let origin =
            variable("PUBLIC_ORIGIN").ok_or(GithubAppConfigError::Missing("PUBLIC_ORIGIN"))?;
        let public_origin =
            reqwest::Url::parse(&origin).map_err(|_| GithubAppConfigError::PublicOrigin)?;
        if public_origin.scheme() != "https" {
            return Err(GithubAppConfigError::PublicOrigin);
        }

        Ok(Some(Self {
            id: id.parse().map_err(|_| GithubAppConfigError::AppId)?,
            key: EncodingKey::from_rsa_pem(&std::fs::read(key_file)?)?,
            webhook_secret: secret.into_bytes(),
            public_origin,
            client: client()?,
            slug: OnceCell::new(),
            tokens: Mutex::new(HashMap::new()),
        }))
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Where a reader of a check follows its Details link to.
    pub fn public_origin(&self) -> &reqwest::Url {
        &self.public_origin
    }

    /// Whether a webhook body carries the signature only the shared secret can make. The
    /// comparison runs in constant time, so a guess learns nothing from how long it took.
    pub fn signed(&self, body: &[u8], header: &str) -> bool {
        signed(&self.webhook_secret, body, header)
    }

    /// The page where an account installs the app. Asked of GitHub once, because the slug
    /// is chosen when the app is registered and no environment variable carries it.
    pub async fn install_url(&self) -> Result<String, GithubAppError> {
        let slug = self
            .slug
            .get_or_try_init(|| async {
                let request = self
                    .client
                    .get(format!("https://{API_HOST}/app"))
                    .bearer_auth(self.jwt()?);
                answer::<AppResponse>(request.send().await?)
                    .await
                    .map(|app| app.slug)
            })
            .await?;

        Ok(format!("https://github.com/apps/{slug}/installations/new"))
    }

    /// A token for one repository of one installation. It is asked for that repository
    /// alone, so it can never reach another repository the same account installed on.
    pub async fn installation_token(
        &self,
        installation_id: i64,
        repository: &str,
    ) -> Result<String, GithubAppError> {
        let key = (installation_id, repository.to_owned());
        let mut tokens = self.tokens.lock().await;
        if let Some(cached) = tokens.get(&key)
            && Timestamp::now() + TOKEN_MARGIN < cached.expires_at
        {
            return Ok(cached.token.clone());
        }

        // The name GitHub scopes a token by is the repository alone, without its owner.
        let name = repository.rsplit('/').next().unwrap_or(repository);
        let request = self
            .client
            .post(format!(
                "https://{API_HOST}/app/installations/{installation_id}/access_tokens"
            ))
            .bearer_auth(self.jwt()?)
            .json(&TokenRequest {
                repositories: [name],
            });
        let response: TokenResponse = answer(request.send().await?).await?;
        let token = response.token.clone();
        tokens.insert(
            key,
            InstallationToken {
                token: response.token,
                expires_at: response.expires_at,
            },
        );

        Ok(token)
    }

    /// A request to the GitHub API, sent with an installation token and its answer decoded.
    pub async fn send<Value: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
    ) -> Result<Value, GithubAppError> {
        answer(request.bearer_auth(token).send().await?).await
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn jwt(&self) -> Result<String, GithubAppError> {
        let now = Timestamp::now().as_second();

        Ok(jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &Claims {
                iat: now - JWT_BACKDATE_SECONDS,
                exp: now + JWT_LIFETIME_SECONDS,
                iss: self.id.to_string(),
            },
            &self.key,
        )?)
    }
}

impl From<GithubAppError> for ForgeReadError {
    /// A budget wait stays a budget wait, so a read through the app is deferred like any
    /// other read rather than failed.
    fn from(error: GithubAppError) -> Self {
        match error {
            GithubAppError::RateLimited { reset } => ForgeReadError::RateLimited {
                host: API_HOST.to_owned(),
                reset,
            },
            other => ForgeReadError::App(other),
        }
    }
}

fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("error.menu/", env!("CARGO_PKG_VERSION")))
        .build()
}

async fn answer<Value: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<Value, GithubAppError> {
    let status = response.status();
    if status.is_client_error() {
        return Err(
            match refusal(API_HOST.to_owned(), &response, status.as_u16()) {
                ForgeReadError::RateLimited { reset, .. } => GithubAppError::RateLimited { reset },
                _ => GithubAppError::Refused {
                    status: status.as_u16(),
                },
            },
        );
    }

    Ok(response.error_for_status()?.json().await?)
}

fn signed(secret: &[u8], body: &[u8], header: &str) -> bool {
    let Some(expected) = header.strip_prefix("sha256=").and_then(hex_bytes) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);

    mac.verify_slice(&expected).is_ok()
}

fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }

    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The example GitHub publishes for checking an implementation of its signature.
    const SECRET: &[u8] = b"It's a Secret to Everybody";
    const BODY: &[u8] = b"Hello, World!";
    const SIGNATURE: &str =
        "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

    #[test]
    fn accepts_the_signature_github_publishes_as_its_example() {
        assert!(signed(SECRET, BODY, SIGNATURE));
    }

    #[test]
    fn refuses_a_changed_body_a_wrong_secret_and_a_malformed_header() {
        assert!(!signed(SECRET, b"Hello, World?", SIGNATURE));
        assert!(!signed(b"another secret", BODY, SIGNATURE));
        assert!(!signed(
            SECRET,
            BODY,
            &SIGNATURE.replacen("sha256=", "sha1=", 1)
        ));
        assert!(!signed(SECRET, BODY, "sha256=75"));
        assert!(!signed(SECRET, BODY, "sha256=zz"));
    }
}
