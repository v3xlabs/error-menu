use std::collections::BTreeMap;
use std::sync::Arc;

use jiff::Timestamp;
use serde::de::DeserializeOwned;

use super::gitea::Gitea;
use super::github::app::{GithubApp, GithubAppError};
use super::github::installation::GithubInstallation;
use super::github::{API_HOST, Github};
use super::gitlab::Gitlab;
use super::{CommitReading, DiscoveredChange, Forge, ForgeKind};
use crate::analysis::ci_checks::CheckRun;
use crate::prelude::*;
use crate::vcs::CommitShaError;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// How many times a forge may redirect a read before it is refused.
const MAX_FORGE_REDIRECTS: usize = 10;
pub(crate) const PAGE_SIZE: &str = "100";

#[derive(Debug, thiserror::Error)]
pub enum ForgeReadError {
    #[error("the remote URL does not identify an owner and repository")]
    RepositoryPath,
    #[error("the remote host needs an explicit forge type")]
    ForgeType,
    #[error("the forge response exceeded its {MAX_RESPONSE_BYTES} byte limit")]
    ResponseTooLarge,
    #[error(
        "{host} refused the request with status {status}; error.menu holds no credential for that host"
    )]
    Unauthorised { host: String, status: u16 },
    #[error("{host} has no request budget left until {reset}")]
    RateLimited { host: String, reset: Timestamp },
    #[error("forge destination is not public: {0}")]
    Destination(#[from] std::io::Error),
    #[error("requesting forge data: {0}")]
    Request(#[from] reqwest::Error),
    #[error("reading forge data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("forge returned an invalid {field} SHA: {source}")]
    Sha {
        field: &'static str,
        #[source]
        source: CommitShaError,
    },
    #[error("{0}")]
    App(GithubAppError),
}

#[derive(Clone)]
pub struct ForgeReader {
    client: reqwest::Client,
    /// One credential per host. A credential for one forge must never travel to another,
    /// because a project's remote is chosen by whoever created the project.
    credentials: BTreeMap<String, Credential>,
    /// A repository the GitHub App is installed on is read with that installation's own
    /// token, which carries its own budget.
    github_app: Option<Arc<GithubApp>>,
}

/// How a forge is told who is asking. A forge offers its own through
/// [`Forge::client_credential`]; an operator offers one for any host through `FORGE_TOKENS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Credential {
    /// A token an account issued, carrying that account's reach.
    Token(String),
    /// An app's own client id and secret, which cost the app's budget rather than a
    /// person's and authorise nothing a signed-out reader cannot already see.
    ClientApp { id: String, secret: String },
}

impl Credential {
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Token(token) => request.bearer_auth(token),
            Self::ClientApp { id, secret } => request.basic_auth(id, Some(secret)),
        }
    }

    /// Names the mechanism without naming the secret, so a misconfigured host is visible
    /// in a log line rather than in a budget that runs out an hour later.
    fn kind(&self) -> &'static str {
        match self {
            Self::Token(_) => "token",
            Self::ClientApp { .. } => "client app",
        }
    }
}

/// One repository on one forge: where that forge's API answers, and the path that names
/// the repository under it.
pub(crate) struct Repository {
    base: ApiBase,
    path: Vec<String>,
    kind: ForgeKind,
}

/// Where a forge answers: the base its API lives under, and the segments every endpoint
/// begins with. Each forge decides its own.
pub(crate) struct ApiBase {
    url: reqwest::Url,
    prefix: Vec<String>,
}

impl ApiBase {
    pub(crate) fn new(url: &str, prefix: &[&str]) -> Result<Self, ForgeReadError> {
        Ok(Self {
            url: reqwest::Url::parse(url).map_err(|_| ForgeReadError::RepositoryPath)?,
            prefix: prefix.iter().map(|segment| (*segment).to_owned()).collect(),
        })
    }
}

/// A repository and the credentialed client that reads it. Forge implementations are
/// handed one of these and never touch the transport themselves.
pub(crate) struct Api<'a> {
    reader: &'a ForgeReader,
    pub(crate) repository: Repository,
    installation_token: Option<String>,
}

impl Api<'_> {
    /// The id of the GitHub App this server writes checks as, when it has one.
    pub(crate) fn github_app_id(&self) -> Option<u64> {
        self.reader.github_app.as_ref().map(|app| app.id())
    }

    pub(crate) async fn get<Value: DeserializeOwned>(
        &self,
        url: reqwest::Url,
    ) -> Result<Value, ForgeReadError> {
        crate::outbound::validate_url(&url)?;
        let host = url.host_str().unwrap_or_default().to_owned();
        let response = self
            .reader
            .authorised(&host, url, self.installation_token.as_deref())
            .send()
            .await?;
        let status = response.status();
        if status.is_client_error() {
            return Err(refusal(host, &response, status.as_u16()));
        }

        decode(response.error_for_status()?).await
    }

    pub(crate) async fn get_with_query<Value: DeserializeOwned>(
        &self,
        mut url: reqwest::Url,
        query: &[(&str, &str)],
    ) -> Result<Value, ForgeReadError> {
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        self.get(url).await
    }
}

impl ForgeReader {
    /// `FORGE_TOKENS` holds `host=token` pairs separated by commas, and a forge may have a
    /// credential of its own to offer. An explicit token for a host wins over it. Without
    /// either, requests to a host stay anonymous and share that host's anonymous budget.
    pub fn new(github_app: Option<Arc<GithubApp>>) -> Result<Self, ForgeReadError> {
        let mut credentials = credentials(std::env::var("FORGE_TOKENS").as_deref().unwrap_or(""));
        for (host, credential) in [
            Github::client_credential(),
            Gitlab::client_credential(),
            Gitea::client_credential(),
        ]
        .into_iter()
        .flatten()
        {
            credentials.entry(host).or_insert(credential);
        }
        for (host, credential) in &credentials {
            tracing::info!(%host, credential = credential.kind(), "forge reads are authenticated");
        }

        Ok(Self {
            client: reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .dns_resolver(Arc::new(crate::outbound::PublicResolver))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() > MAX_FORGE_REDIRECTS
                        || crate::outbound::validate_url(attempt.url()).is_err()
                    {
                        return attempt.error(std::io::Error::other(
                            "the forge redirected somewhere the read may not go",
                        ));
                    }
                    attempt.follow()
                }))
                .referer(false)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(15))
                .user_agent(concat!("error.menu/", env!("CARGO_PKG_VERSION")))
                .build()?,
            credentials,
            github_app,
        })
    }

    /// The changes a forge keeps on top of git: their numbers, their heads, and the state,
    /// titles and people that live nowhere in the repository. What git itself answers, the
    /// mirror answers, and this never asks for it.
    pub async fn changes(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
        installation: Option<GithubInstallation>,
    ) -> Result<Vec<DiscoveredChange>, ForgeReadError> {
        let api = self.api(remote, configured_kind, installation).await?;

        match api.repository.kind {
            ForgeKind::Github => Github::changes(&api).await,
            ForgeKind::Gitlab => Gitlab::changes(&api).await,
            ForgeKind::Gitea | ForgeKind::Forgejo => Gitea::changes(&api).await,
            ForgeKind::Auto => Err(ForgeReadError::ForgeType),
        }
    }

    /// What the forge says about a commit: the signature verdict, and which accounts wrote
    /// it. error.menu does not check the cryptography itself, so a forge that cannot answer
    /// leaves the verdict absent rather than turning "unknown" into "bad". The accounts are
    /// the join between an address a commit was written with and a forge login.
    pub async fn read_commit(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
        installation: Option<GithubInstallation>,
        head: &CommitSha,
    ) -> Result<CommitReading, ForgeReadError> {
        let api = self.api(remote, configured_kind, installation).await?;

        match api.repository.kind {
            ForgeKind::Github => Github::read_commit(&api, head).await,
            ForgeKind::Gitlab => Gitlab::read_commit(&api, head).await,
            ForgeKind::Gitea | ForgeKind::Forgejo => Gitea::read_commit(&api, head).await,
            ForgeKind::Auto => Err(ForgeReadError::ForgeType),
        }
    }

    pub async fn check_runs(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
        installation: Option<GithubInstallation>,
        head: &CommitSha,
    ) -> Result<Vec<CheckRun>, ForgeReadError> {
        let api = self.api(remote, configured_kind, installation).await?;

        match api.repository.kind {
            ForgeKind::Github => Github::check_runs(&api, head).await,
            ForgeKind::Gitlab => Gitlab::check_runs(&api, head).await,
            ForgeKind::Gitea | ForgeKind::Forgejo => Gitea::check_runs(&api, head).await,
            ForgeKind::Auto => Err(ForgeReadError::ForgeType),
        }
    }

    async fn api(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
        installation: Option<GithubInstallation>,
    ) -> Result<Api<'_>, ForgeReadError> {
        let repository = Repository::from_remote(remote, configured_kind)?;
        let installation_token = match (&self.github_app, installation) {
            (Some(app), Some(installation)) if repository.on_public_github() => {
                match app
                    .installation_token(installation.id, &repository.project_path())
                    .await
                {
                    Ok(token) => Some(token),
                    // An install removed while error.menu was not listening still has its
                    // row. The repository reads as it did before it was installed.
                    Err(error @ GithubAppError::Refused { .. }) => {
                        tracing::warn!(%error, installation = installation.id, "reading without the installation");
                        None
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            _ => None,
        };

        Ok(Api {
            reader: self,
            repository,
            installation_token,
        })
    }

    /// A request carrying the credential its host is owed, if any. An installation token
    /// is only ever owed to the GitHub API host, whatever host the URL names.
    fn authorised(
        &self,
        host: &str,
        url: reqwest::Url,
        installation_token: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let request = self.client.get(url);

        match (installation_token, self.credentials.get(host)) {
            (Some(token), _) if host == API_HOST => request.bearer_auth(token),
            (_, Some(credential)) => credential.apply(request),
            _ => request,
        }
    }
}

impl Repository {
    pub(crate) fn from_remote(
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
    ) -> Result<Self, ForgeReadError> {
        let remote =
            reqwest::Url::parse(remote.as_str()).map_err(|_| ForgeReadError::RepositoryPath)?;
        let host = remote.host_str().ok_or(ForgeReadError::RepositoryPath)?;
        let mut path = remote
            .path_segments()
            .ok_or(ForgeReadError::RepositoryPath)?
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let Some(name) = path.pop() else {
            return Err(ForgeReadError::RepositoryPath);
        };
        let name = name.strip_suffix(".git").unwrap_or(&name).to_owned();
        if name.is_empty() || path.is_empty() {
            return Err(ForgeReadError::RepositoryPath);
        }
        path.push(name);

        let kind = match configured_kind {
            ForgeKind::Auto => match host {
                "github.com" => ForgeKind::Github,
                "gitlab.com" => ForgeKind::Gitlab,
                _ => return Err(ForgeReadError::ForgeType),
            },
            kind => kind,
        };

        // GitLab is the only supported forge that nests a project under several groups.
        if kind != ForgeKind::Gitlab && path.len() != 2 {
            return Err(ForgeReadError::RepositoryPath);
        }

        let authority = match remote.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        let base = match kind {
            ForgeKind::Github => Github::api_base(host, &authority)?,
            ForgeKind::Gitlab => Gitlab::api_base(host, &authority)?,
            ForgeKind::Gitea | ForgeKind::Forgejo => Gitea::api_base(host, &authority)?,
            ForgeKind::Auto => return Err(ForgeReadError::ForgeType),
        };

        Ok(Self { base, path, kind })
    }

    pub(crate) fn project_path(&self) -> String {
        self.path.join("/")
    }

    /// Whether the repository is on github.com, the one host a registered app answers for.
    pub(crate) fn on_public_github(&self) -> bool {
        self.kind == ForgeKind::Github && self.base.url.host_str() == Some(API_HOST)
    }

    pub(crate) fn endpoint(&self, segments: &[&str]) -> reqwest::Url {
        let mut url = self.base.url.clone();
        let mut path = url
            .path_segments_mut()
            .expect("forge API base URL has a path");
        for segment in self
            .base
            .prefix
            .iter()
            .map(String::as_str)
            .chain(segments.iter().copied())
        {
            path.push(segment);
        }
        drop(path);
        url
    }

    pub(crate) fn repository_url(&self, prefix: &str, suffix: &[&str]) -> reqwest::Url {
        let mut url = self.base.url.clone();
        let mut owned = url
            .path_segments_mut()
            .expect("forge API base URL has a path");
        for segment in self.base.prefix.iter().map(String::as_str) {
            owned.push(segment);
        }
        owned.push(prefix);
        for segment in self.path.iter().map(String::as_str) {
            owned.push(segment);
        }
        for segment in suffix {
            owned.push(segment);
        }
        drop(owned);
        url
    }
}

/// A forge that is out of budget and a forge that will not answer without a credential
/// both answer 403. Only the exhausted budget is worth waiting for, and the forge says
/// when the wait ends, so that answer must survive as more than a status code.
pub(crate) fn refusal(host: String, response: &reqwest::Response, status: u16) -> ForgeReadError {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let exhausted = header("x-ratelimit-remaining") == Some("0");
    let reset = header("x-ratelimit-reset")
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|seconds| Timestamp::from_second(seconds).ok())
        .or_else(|| {
            header("retry-after")
                .and_then(|value| value.parse::<i64>().ok())
                .map(|seconds| {
                    Timestamp::now() + std::time::Duration::from_secs(seconds.max(0) as u64)
                })
        });
    match reset.filter(|_| exhausted || status == 429) {
        Some(reset) => ForgeReadError::RateLimited { host, reset },
        None => ForgeReadError::Unauthorised { host, status },
    }
}

fn credentials(configured: &str) -> BTreeMap<String, Credential> {
    configured
        .split(',')
        .filter_map(|entry| entry.split_once('='))
        .map(|(host, token)| (host.trim().to_owned(), token.trim().to_owned()))
        .filter(|(host, token)| !host.is_empty() && !token.is_empty())
        .map(|(host, token)| (host, Credential::Token(token)))
        .collect()
}

async fn decode<Value: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<Value, ForgeReadError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ForgeReadError::ResponseTooLarge);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(ForgeReadError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }

    Ok(serde_json::from_slice(&body)?)
}

/// A forge reports an absent merge commit as null or as an empty string, and neither
/// means the change was merged.
pub(crate) fn optional_sha(value: Option<&str>) -> Option<CommitSha> {
    value.and_then(|value| CommitSha::new(value).ok())
}

pub(crate) fn sha(field: &'static str, value: &str) -> Result<CommitSha, ForgeReadError> {
    CommitSha::new(value).map_err(|source| ForgeReadError::Sha { field, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(remote: &str, kind: ForgeKind) -> Repository {
        Repository::from_remote(&RemoteUrl::new(remote).unwrap(), kind).unwrap()
    }

    fn response(status: u16, headers: &[(&str, &str)]) -> reqwest::Response {
        let mut built = poem::http::Response::builder().status(status);
        for (name, value) in headers {
            built = built.header(*name, *value);
        }

        reqwest::Response::from(built.body(Vec::new()).expect("a response"))
    }

    fn response_with_body(body: Vec<u8>) -> reqwest::Response {
        reqwest::Response::from(
            poem::http::Response::builder()
                .status(200)
                .body(body)
                .expect("a response"),
        )
    }

    #[test]
    fn an_exhausted_budget_is_told_apart_from_a_refusal_and_keeps_its_reset() {
        let exhausted = refusal(
            "api.github.com".to_owned(),
            &response(
                403,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "3600"),
                ],
            ),
            403,
        );
        assert!(matches!(
            exhausted,
            ForgeReadError::RateLimited { reset, .. } if reset == Timestamp::from_second(3600).unwrap()
        ));

        let refused = refusal(
            "api.github.com".to_owned(),
            &response(403, &[("x-ratelimit-remaining", "58")]),
            403,
        );
        assert!(matches!(
            refused,
            ForgeReadError::Unauthorised { status: 403, .. }
        ));
    }

    #[tokio::test]
    async fn forge_request_rejects_private_literal_before_connecting() {
        let reader = ForgeReader {
            client: crate::outbound::client().unwrap(),
            credentials: BTreeMap::new(),
            github_app: None,
        };
        let api = reader
            .api(
                &RemoteUrl::new("https://github.com/owner/repo").unwrap(),
                ForgeKind::Github,
                None,
            )
            .await
            .unwrap();
        let result = api
            .get::<serde_json::Value>(reqwest::Url::parse("https://127.0.0.1/secret").unwrap())
            .await;
        assert!(matches!(result, Err(ForgeReadError::Destination(_))));
    }

    #[test]
    fn a_credential_is_kept_for_its_own_host_only() {
        let configured = credentials(" github.com = token-one , forge.example = token-two ,=x, y=");

        assert_eq!(
            configured.get("github.com"),
            Some(&Credential::Token("token-one".to_owned()))
        );
        assert_eq!(
            configured.get("forge.example"),
            Some(&Credential::Token("token-two".to_owned()))
        );
        assert_eq!(configured.len(), 2);
    }

    #[test]
    fn public_github_reads_carry_the_installation_or_the_oauth_app_and_no_account() {
        let reader = ForgeReader {
            client: reqwest::Client::new(),
            credentials: BTreeMap::from([(
                crate::forge::github::API_HOST.to_owned(),
                Credential::ClientApp {
                    id: "app-id".to_owned(),
                    secret: "app-secret".to_owned(),
                },
            )]),
            github_app: None,
        };
        let authorization = |host: &str, url: &str, installation_token: Option<&str>| {
            reader
                .authorised(host, reqwest::Url::parse(url).unwrap(), installation_token)
                .build()
                .unwrap()
                .headers()
                .get("authorization")
                .map(|value| value.to_str().unwrap().to_owned())
        };

        assert_eq!(
            authorization(API_HOST, "https://api.github.com/repos/o/r", None).as_deref(),
            Some("Basic YXBwLWlkOmFwcC1zZWNyZXQ=")
        );
        assert_eq!(
            authorization(API_HOST, "https://api.github.com/repos/o/r", Some("ghs_x")).as_deref(),
            Some("Bearer ghs_x")
        );
        assert_eq!(
            authorization("github.com", "https://github.com/o/r", None),
            None
        );
        assert_eq!(
            authorization("github.com", "https://github.com/o/r", Some("ghs_x")),
            None
        );
    }

    #[tokio::test]
    async fn decodes_responses_up_to_eight_mebibytes_and_rejects_larger_ones() {
        let mut accepted = Vec::with_capacity(MAX_RESPONSE_BYTES);
        accepted.push(b'[');
        accepted.resize(MAX_RESPONSE_BYTES - 1, b' ');
        accepted.push(b']');
        let value: Vec<serde_json::Value> = decode(response_with_body(accepted))
            .await
            .expect("the eight-mebibyte response is accepted");
        assert!(value.is_empty());

        let oversized = vec![b' '; MAX_RESPONSE_BYTES + 1];
        assert!(matches!(
            decode::<serde_json::Value>(response_with_body(oversized)).await,
            Err(ForgeReadError::ResponseTooLarge)
        ));
    }

    #[test]
    fn recognises_public_github_from_an_https_remote() {
        let subject = repository(
            "https://github.com/open-lavatory/open-lavatory",
            ForgeKind::Auto,
        );

        assert_eq!(subject.kind, ForgeKind::Github);
        assert_eq!(subject.project_path(), "open-lavatory/open-lavatory");
    }

    #[test]
    fn requires_a_forge_type_for_an_unknown_host() {
        let remote = RemoteUrl::new("https://forge.example.invalid/group/project.git").unwrap();

        assert!(matches!(
            Repository::from_remote(&remote, ForgeKind::Auto),
            Err(ForgeReadError::ForgeType)
        ));
    }

    #[test]
    fn uses_the_configured_forge_for_an_unknown_host() {
        let subject = repository(
            "https://forge.example.invalid/group/project.git",
            ForgeKind::Forgejo,
        );

        assert_eq!(subject.kind, ForgeKind::Forgejo);
        assert_eq!(subject.project_path(), "group/project");
    }

    #[test]
    fn public_github_reads_its_own_api_host() {
        let subject = repository(
            "https://github.com/open-lavatory/open-lavatory",
            ForgeKind::Auto,
        );

        assert_eq!(
            subject.repository_url("repos", &[]).as_str(),
            "https://api.github.com/repos/open-lavatory/open-lavatory"
        );
        assert_eq!(
            subject.repository_url("repos", &["pulls"]).as_str(),
            "https://api.github.com/repos/open-lavatory/open-lavatory/pulls"
        );
    }

    #[test]
    fn github_enterprise_reads_the_api_under_its_own_host() {
        let subject = repository(
            "https://git.example.invalid/team/service",
            ForgeKind::Github,
        );

        assert_eq!(
            subject.repository_url("repos", &[]).as_str(),
            "https://git.example.invalid/api/v3/repos/team/service"
        );
    }

    #[test]
    fn gitea_reads_the_api_under_its_own_host() {
        let subject = repository("https://codeberg.org/team/service.git", ForgeKind::Gitea);

        assert_eq!(
            subject
                .repository_url("repos", &["branches", "main"])
                .as_str(),
            "https://codeberg.org/api/v1/repos/team/service/branches/main"
        );
    }

    #[test]
    fn gitlab_encodes_a_nested_group_path_as_one_segment() {
        let subject = repository("https://gitlab.com/group/subgroup/service", ForgeKind::Auto);

        assert_eq!(subject.kind, ForgeKind::Gitlab);
        assert_eq!(
            subject
                .endpoint(&["projects", &subject.project_path()])
                .as_str(),
            "https://gitlab.com/api/v4/projects/group%2Fsubgroup%2Fservice"
        );
    }

    #[test]
    fn a_nested_path_is_not_a_github_repository() {
        let remote = RemoteUrl::new("https://github.com/owner/group/service").unwrap();

        assert!(matches!(
            Repository::from_remote(&remote, ForgeKind::Auto),
            Err(ForgeReadError::RepositoryPath)
        ));
    }

    #[test]
    fn an_explicit_port_stays_in_the_api_url() {
        let subject = repository(
            "https://forge.example.invalid:8443/team/service",
            ForgeKind::Forgejo,
        );

        assert_eq!(
            subject.repository_url("repos", &[]).as_str(),
            "https://forge.example.invalid:8443/api/v1/repos/team/service"
        );
    }
}
