use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{
    ChangeState, CommitReading, DiscoveredBranch, DiscoveredChange, DiscoveredProject, ForgeAccount,
    ForgeKind, ForgeMetadata,
};
use crate::analysis::ci_checks::{CheckConclusion, CheckRun, CheckStatus};
use crate::person::{Person, PersonRole, Signature};
use crate::vcs::{CommitSha, CommitShaError, RemoteUrl};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const PAGE_SIZE: &str = "100";

#[derive(Debug, thiserror::Error)]
pub enum ForgeReadError {
    #[error("the remote URL does not identify an owner and repository")]
    RepositoryPath,
    #[error("the remote host needs an explicit forge type")]
    ForgeType,
    #[error("the forge response exceeded its {MAX_RESPONSE_BYTES} byte limit")]
    ResponseTooLarge,
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
}

pub struct ForgeReader {
    client: reqwest::Client,
}

struct Repository {
    api: reqwest::Url,
    prefix: Vec<String>,
    path: Vec<String>,
    kind: ForgeKind,
}

impl ForgeReader {
    pub fn new() -> Result<Self, ForgeReadError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .user_agent(concat!("error.menu/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }

    pub async fn discover(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
    ) -> Result<DiscoveredProject, ForgeReadError> {
        let repository = Repository::from_remote(remote, configured_kind)?;

        match repository.kind {
            ForgeKind::Github => self.github(&repository).await,
            ForgeKind::Gitlab => self.gitlab(&repository).await,
            ForgeKind::Gitea | ForgeKind::Forgejo => self.gitea(&repository).await,
            ForgeKind::Auto => Err(ForgeReadError::ForgeType),
        }
    }

    /// What the forge says about a commit: the signature verdict, and which accounts wrote
    /// it. error.menu does not check the cryptography itself, so a forge that cannot answer
    /// leaves the verdict absent rather than turning "unknown" into "bad". The accounts are
    /// the join between an address a commit was written with and a forge login, and GitLab's
    /// signature endpoint carries none, so only GitHub, Gitea and Forgejo report them.
    pub async fn read_commit(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
        sha: &CommitSha,
    ) -> Result<CommitReading, ForgeReadError> {
        let repository = Repository::from_remote(remote, configured_kind)?;

        match repository.kind {
            ForgeKind::Github | ForgeKind::Gitea | ForgeKind::Forgejo => {
                let commit: CommitDetail = self
                    .get(repository.repository_url("repos", &["commits", sha.as_str()]))
                    .await?;
                let verification = commit.commit.verification;

                Ok(CommitReading {
                    signature: Signature {
                        present: verification
                            .as_ref()
                            .is_some_and(|value| value.signature.is_some()),
                        verified: verification.as_ref().map(|value| value.verified),
                        signer: verification
                            .as_ref()
                            .and_then(|value| value.signer.as_ref())
                            .map(|signer| signer.login.clone()),
                        reason: verification.and_then(|value| value.reason),
                    },
                    accounts: [
                        account_of(commit.commit.author, commit.author),
                        account_of(commit.commit.committer, commit.committer),
                    ]
                    .into_iter()
                    .flatten()
                    .collect(),
                })
            }
            ForgeKind::Gitlab => {
                let endpoint = repository.endpoint(&[
                    "projects",
                    &repository.project_path(),
                    "repository",
                    "commits",
                    sha.as_str(),
                    "signature",
                ]);
                match self.get::<GitlabSignature>(endpoint).await {
                    Ok(signature) => Ok(CommitReading {
                        signature: Signature {
                            present: true,
                            verified: Some(signature.verification_status == "verified"),
                            signer: signature.gpg_key_user_name,
                            reason: Some(signature.verification_status),
                        },
                        accounts: Vec::new(),
                    }),
                    // GitLab answers 404 when the commit carries no signature at all.
                    Err(ForgeReadError::Request(_)) => Ok(CommitReading::default()),
                    Err(error) => Err(error),
                }
            }
            ForgeKind::Auto => Err(ForgeReadError::ForgeType),
        }
    }
    pub async fn check_runs(
        &self,
        remote: &RemoteUrl,
        configured_kind: ForgeKind,
        sha: &CommitSha,
    ) -> Result<Vec<CheckRun>, ForgeReadError> {
        let repository = Repository::from_remote(remote, configured_kind)?;

        match repository.kind {
            ForgeKind::Github => self.github_check_runs(&repository, sha).await,
            ForgeKind::Gitlab => self.gitlab_check_runs(&repository, sha).await,
            ForgeKind::Gitea | ForgeKind::Forgejo => self.gitea_check_runs(&repository, sha).await,
            ForgeKind::Auto => Err(ForgeReadError::ForgeType),
        }
    }

    async fn github_check_runs(
        &self,
        repository: &Repository,
        sha: &CommitSha,
    ) -> Result<Vec<CheckRun>, ForgeReadError> {
        let response: GithubCheckRuns = self
            .get_with_query(
                repository.repository_url("repos", &["commits", sha.as_str(), "check-runs"]),
                &[("per_page", PAGE_SIZE)],
            )
            .await?;

        Ok(response
            .check_runs
            .into_iter()
            .map(GithubCheckRun::into_check_run)
            .collect())
    }

    async fn gitlab_check_runs(
        &self,
        repository: &Repository,
        sha: &CommitSha,
    ) -> Result<Vec<CheckRun>, ForgeReadError> {
        let project: GitlabProject = self
            .get(repository.endpoint(&["projects", &repository.project_path()]))
            .await?;
        let project_id = project.id.to_string();
        let pipelines: Vec<GitlabPipeline> = self
            .get_with_query(
                repository.endpoint(&["projects", &project_id, "pipelines"]),
                &[("sha", sha.as_str()), ("per_page", PAGE_SIZE)],
            )
            .await?;
        let mut check_runs = Vec::new();

        for pipeline in pipelines {
            let pipeline_id = pipeline.id.to_string();
            let jobs: Vec<GitlabJob> = self
                .get_with_query(
                    repository.endpoint(&["projects", &project_id, "pipelines", &pipeline_id, "jobs"]),
                    &[("per_page", PAGE_SIZE)],
                )
                .await?;
            check_runs.extend(jobs.into_iter().map(|job| job.into_check_run(repository, &project_id)));
        }

        Ok(check_runs)
    }

    async fn gitea_check_runs(
        &self,
        repository: &Repository,
        sha: &CommitSha,
    ) -> Result<Vec<CheckRun>, ForgeReadError> {
        let response: GiteaCombinedStatus = self
            .get(repository.repository_url("repos", &["commits", sha.as_str(), "status"]))
            .await?;

        Ok(response
            .statuses
            .into_iter()
            .map(GiteaCommitStatus::into_check_run)
            .collect())
    }

    async fn github(&self, repository: &Repository) -> Result<DiscoveredProject, ForgeReadError> {
        let project: GithubProject = self.get(repository.repository_url("repos", &[])).await?;
        let branch: GithubBranch = self
            .get(repository.repository_url("repos", &["branches", &project.default_branch]))
            .await?;
        let pulls: Vec<GithubPull> = self
            .get_with_query(
                repository.repository_url("repos", &["pulls"]),
                &[
                    ("state", "all"),
                    ("sort", "updated"),
                    ("direction", "desc"),
                    ("per_page", PAGE_SIZE),
                ],
            )
            .await?;

        Ok(DiscoveredProject {
            default_branch: DiscoveredBranch {
                name: project.default_branch,
                head: sha("default branch", &branch.commit.sha)?,
            },
            changes: pulls
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }

    async fn gitlab(&self, repository: &Repository) -> Result<DiscoveredProject, ForgeReadError> {
        let project: GitlabProject = self
            .get(repository.endpoint(&["projects", &repository.project_path()]))
            .await?;
        let project_id = project.id.to_string();
        let branch: GitlabBranch = self
            .get(repository.endpoint(&[
                "projects",
                &project_id,
                "repository",
                "branches",
                &project.default_branch,
            ]))
            .await?;
        let changes: Vec<GitlabChange> = self
            .get_with_query(
                repository.endpoint(&["projects", &project_id, "merge_requests"]),
                &[
                    ("scope", "all"),
                    ("order_by", "updated_at"),
                    ("sort", "desc"),
                    ("per_page", PAGE_SIZE),
                ],
            )
            .await?;

        Ok(DiscoveredProject {
            default_branch: DiscoveredBranch {
                name: project.default_branch,
                head: sha("default branch", &branch.commit.id)?,
            },
            changes: changes
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }

    async fn gitea(&self, repository: &Repository) -> Result<DiscoveredProject, ForgeReadError> {
        let project: GiteaProject = self.get(repository.repository_url("repos", &[])).await?;
        let branch: GiteaBranch = self
            .get(repository.repository_url("repos", &["branches", &project.default_branch]))
            .await?;
        let changes: Vec<GiteaChange> = self
            .get_with_query(
                repository.repository_url("repos", &["pulls"]),
                &[
                    ("state", "all"),
                    ("sort", "recentupdate"),
                    ("limit", PAGE_SIZE),
                ],
            )
            .await?;

        Ok(DiscoveredProject {
            default_branch: DiscoveredBranch {
                name: project.default_branch,
                head: sha("default branch", &branch.commit.id)?,
            },
            changes: changes
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }

    async fn get<Value: DeserializeOwned>(
        &self,
        url: reqwest::Url,
    ) -> Result<Value, ForgeReadError> {
        let response = self.client.get(url).send().await?.error_for_status()?;
        decode(response).await
    }

    async fn get_with_query<Value: DeserializeOwned>(
        &self,
        mut url: reqwest::Url,
        query: &[(&str, &str)],
    ) -> Result<Value, ForgeReadError> {
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        self.get(url).await
    }
}

impl Repository {
    fn from_remote(remote: &RemoteUrl, configured_kind: ForgeKind) -> Result<Self, ForgeReadError> {
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
        // The public GitHub REST API answers on its own host. GitHub Enterprise Server
        // serves the same API under the repository host instead.
        let (api, prefix) = match kind {
            ForgeKind::Github if host == "github.com" => ("https://api.github.com/", Vec::new()),
            ForgeKind::Github => (&*format!("https://{authority}/"), vec!["api", "v3"]),
            ForgeKind::Gitlab => (&*format!("https://{authority}/"), vec!["api", "v4"]),
            ForgeKind::Gitea | ForgeKind::Forgejo => {
                (&*format!("https://{authority}/"), vec!["api", "v1"])
            }
            ForgeKind::Auto => return Err(ForgeReadError::ForgeType),
        };
        let api = reqwest::Url::parse(api).map_err(|_| ForgeReadError::RepositoryPath)?;

        Ok(Self {
            api,
            prefix: prefix.into_iter().map(str::to_owned).collect(),
            path,
            kind,
        })
    }

    fn project_path(&self) -> String {
        self.path.join("/")
    }

    fn endpoint(&self, segments: &[&str]) -> reqwest::Url {
        let mut url = self.api.clone();
        let mut path = url
            .path_segments_mut()
            .expect("forge API base URL has a path");
        for segment in self
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

    fn repository_url(&self, prefix: &str, suffix: &[&str]) -> reqwest::Url {
        let mut url = self.api.clone();
        let mut owned = url
            .path_segments_mut()
            .expect("forge API base URL has a path");
        for segment in self.prefix.iter().map(String::as_str) {
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
fn optional_sha(value: Option<&str>) -> Option<CommitSha> {
    value.and_then(|value| CommitSha::new(value).ok())
}

fn sha(field: &'static str, value: &str) -> Result<CommitSha, ForgeReadError> {
    CommitSha::new(value).map_err(|source| ForgeReadError::Sha { field, source })
}

#[derive(Deserialize)]
struct GithubCheckRuns {
    #[serde(default)]
    check_runs: Vec<GithubCheckRun>,
}

#[derive(Deserialize)]
struct GithubCheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl GithubCheckRun {
    fn into_check_run(self) -> CheckRun {
        let log_excerpt_ref = self.url;
        let url = self.details_url.or_else(|| log_excerpt_ref.clone());

        CheckRun {
            name: self.name,
            status: check_status(&self.status),
            conclusion: self.conclusion.as_deref().and_then(check_conclusion),
            url,
            log_excerpt_ref,
        }
    }
}

#[derive(Deserialize)]
struct GitlabPipeline {
    id: u64,
}

#[derive(Deserialize)]
struct GitlabJob {
    id: u64,
    name: String,
    status: String,
    #[serde(default)]
    web_url: Option<String>,
}

impl GitlabJob {
    fn into_check_run(self, repository: &Repository, project_id: &str) -> CheckRun {
        let job_id = self.id.to_string();
        let log_excerpt_ref = repository
            .endpoint(&["projects", project_id, "jobs", &job_id, "trace"])
            .to_string();

        CheckRun {
            name: self.name,
            status: check_status(&self.status),
            conclusion: check_conclusion(&self.status),
            url: self.web_url,
            log_excerpt_ref: Some(log_excerpt_ref),
        }
    }
}

#[derive(Deserialize)]
struct GiteaCombinedStatus {
    #[serde(default)]
    statuses: Vec<GiteaCommitStatus>,
}

#[derive(Deserialize)]
struct GiteaCommitStatus {
    context: String,
    status: String,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl GiteaCommitStatus {
    fn into_check_run(self) -> CheckRun {
        let log_excerpt_ref = self.url;
        let url = self.target_url.or_else(|| log_excerpt_ref.clone());

        CheckRun {
            name: self.context,
            status: check_status(&self.status),
            conclusion: check_conclusion(&self.status),
            url,
            log_excerpt_ref,
        }
    }
}

fn check_status(value: &str) -> CheckStatus {
    match value {
        "queued" | "pending" | "created" | "scheduled" => CheckStatus::Queued,
        "completed" | "success" | "failure" | "failed" | "error" | "warning" | "neutral" | "skipped" | "cancelled"
        | "canceled" | "timed_out" | "action_required" | "stale" => CheckStatus::Completed,
        _ => CheckStatus::InProgress,
    }
}

fn check_conclusion(value: &str) -> Option<CheckConclusion> {
    match value {
        "success" => Some(CheckConclusion::Success),
        "failure" | "failed" | "error" => Some(CheckConclusion::Failure),
        "warning" | "neutral" => Some(CheckConclusion::Neutral),
        "skipped" => Some(CheckConclusion::Skipped),
        "cancelled" | "canceled" => Some(CheckConclusion::Cancelled),
        "timed_out" => Some(CheckConclusion::TimedOut),
        "action_required" => Some(CheckConclusion::ActionRequired),
        "stale" => Some(CheckConclusion::Stale),
        _ => None,
    }
}

#[derive(Deserialize)]
struct GithubProject {
    default_branch: String,
}

#[derive(Deserialize)]
struct GithubBranch {
    commit: GithubCommit,
}

#[derive(Deserialize)]
struct GithubCommit {
    sha: String,
}

#[derive(Deserialize)]
struct GithubPull {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    state: String,
    merged_at: Option<String>,
    merge_commit_sha: Option<String>,
    user: GithubUser,
    #[serde(default)]
    requested_reviewers: Vec<GithubUser>,
    base: GithubReference,
    head: GithubReference,
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GithubReference {
    sha: String,
    #[serde(rename = "ref")]
    name: String,
}

impl GithubUser {
    fn person(self, role: PersonRole) -> Person {
        Person {
            role,
            name: None,
            email: None,
            login: Some(self.login),
            avatar_url: self.avatar_url,
        }
    }
}

impl TryFrom<GithubPull> for DiscoveredChange {
    type Error = ForgeReadError;

    fn try_from(change: GithubPull) -> Result<Self, Self::Error> {
        let state = if change.merged_at.is_some() {
            ChangeState::Merged
        } else if change.state == "closed" {
            ChangeState::Closed
        } else {
            ChangeState::Open
        };
        let mut people = vec![change.user.person(PersonRole::Submitter)];
        people.extend(
            change
                .requested_reviewers
                .into_iter()
                .map(|user| user.person(PersonRole::Reviewer)),
        );

        Ok(Self {
            number: change.number,
            fetch_ref: super::change_fetch_ref(ForgeKind::Github, change.number),
            base: sha("pull request base", &change.base.sha)?,
            head: sha("pull request head", &change.head.sha)?,
            metadata: ForgeMetadata {
                title: Some(change.title),
                body: change.body,
                author: people.first().and_then(|person| person.login.clone()),
                url: Some(change.html_url),
                base_ref: Some(change.base.name),
                head_ref: Some(change.head.name),
                state: Some(state),
                merge_commit: optional_sha(change.merge_commit_sha.as_deref()),
                people: crate::person::deduplicate(people),
                signature: Signature::default(),
            },
        })
    }
}

#[derive(Deserialize)]
struct GitlabProject {
    id: u64,
    default_branch: String,
}

#[derive(Deserialize)]
struct GitlabBranch {
    commit: GitlabCommit,
}

#[derive(Deserialize)]
struct GitlabCommit {
    id: String,
}

#[derive(Deserialize)]
struct GitlabChange {
    iid: u64,
    web_url: String,
    title: String,
    description: Option<String>,
    state: String,
    merge_commit_sha: Option<String>,
    author: GitlabUser,
    #[serde(default)]
    reviewers: Vec<GitlabUser>,
    target_branch: String,
    source_branch: String,
    sha: String,
    diff_refs: GitlabDiffReferences,
}

#[derive(Deserialize)]
struct GitlabUser {
    username: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GitlabDiffReferences {
    base_sha: String,
}

impl GitlabUser {
    fn person(self, role: PersonRole) -> Person {
        Person {
            role,
            name: self.name,
            email: None,
            login: Some(self.username),
            avatar_url: self.avatar_url,
        }
    }
}

impl TryFrom<GitlabChange> for DiscoveredChange {
    type Error = ForgeReadError;

    fn try_from(change: GitlabChange) -> Result<Self, Self::Error> {
        let state = match change.state.as_str() {
            "merged" => ChangeState::Merged,
            "closed" | "locked" => ChangeState::Closed,
            _ => ChangeState::Open,
        };
        let mut people = vec![change.author.person(PersonRole::Submitter)];
        people.extend(
            change
                .reviewers
                .into_iter()
                .map(|user| user.person(PersonRole::Reviewer)),
        );

        Ok(Self {
            number: change.iid,
            fetch_ref: super::change_fetch_ref(ForgeKind::Gitlab, change.iid),
            base: sha("merge request base", &change.diff_refs.base_sha)?,
            head: sha("merge request head", &change.sha)?,
            metadata: ForgeMetadata {
                title: Some(change.title),
                body: change.description,
                author: people.first().and_then(|person| person.login.clone()),
                url: Some(change.web_url),
                base_ref: Some(change.target_branch),
                head_ref: Some(change.source_branch),
                state: Some(state),
                merge_commit: optional_sha(change.merge_commit_sha.as_deref()),
                people: crate::person::deduplicate(people),
                signature: Signature::default(),
            },
        })
    }
}

#[derive(Deserialize)]
struct GiteaProject {
    default_branch: String,
}

#[derive(Deserialize)]
struct GiteaBranch {
    commit: GiteaCommit,
}

#[derive(Deserialize)]
struct GiteaCommit {
    id: String,
}

#[derive(Deserialize)]
struct GiteaChange {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    state: String,
    #[serde(default)]
    merged: bool,
    merge_commit_sha: Option<String>,
    user: GiteaUser,
    #[serde(default)]
    requested_reviewers: Vec<GiteaUser>,
    base: GiteaReference,
    head: GiteaReference,
}

#[derive(Deserialize)]
struct GiteaUser {
    login: String,
    full_name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GiteaReference {
    sha: String,
    #[serde(rename = "ref")]
    name: String,
}

impl GiteaUser {
    fn person(self, role: PersonRole) -> Person {
        Person {
            role,
            name: self.full_name.filter(|name| !name.is_empty()),
            email: None,
            login: Some(self.login),
            avatar_url: self.avatar_url,
        }
    }
}

impl TryFrom<GiteaChange> for DiscoveredChange {
    type Error = ForgeReadError;

    fn try_from(change: GiteaChange) -> Result<Self, Self::Error> {
        let state = if change.merged {
            ChangeState::Merged
        } else if change.state == "closed" {
            ChangeState::Closed
        } else {
            ChangeState::Open
        };
        let mut people = vec![change.user.person(PersonRole::Submitter)];
        people.extend(
            change
                .requested_reviewers
                .into_iter()
                .map(|user| user.person(PersonRole::Reviewer)),
        );

        Ok(Self {
            number: change.number,
            fetch_ref: super::change_fetch_ref(ForgeKind::Gitea, change.number),
            base: sha("pull request base", &change.base.sha)?,
            head: sha("pull request head", &change.head.sha)?,
            metadata: ForgeMetadata {
                title: Some(change.title),
                body: change.body,
                author: people.first().and_then(|person| person.login.clone()),
                url: Some(change.html_url),
                base_ref: Some(change.base.name),
                head_ref: Some(change.head.name),
                state: Some(state),
                merge_commit: optional_sha(change.merge_commit_sha.as_deref()),
                people: crate::person::deduplicate(people),
                signature: Signature::default(),
            },
        })
    }
}

#[derive(Deserialize)]
struct CommitDetail {
    commit: CommitBody,
    author: Option<CommitAccount>,
    committer: Option<CommitAccount>,
}

#[derive(Deserialize)]
struct CommitBody {
    verification: Option<CommitVerification>,
    author: Option<CommitIdentity>,
    committer: Option<CommitIdentity>,
}

#[derive(Deserialize)]
struct CommitIdentity {
    email: Option<String>,
}

#[derive(Deserialize)]
struct CommitAccount {
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct CommitVerification {
    verified: bool,
    reason: Option<String>,
    signature: Option<String>,
    signer: Option<CommitSigner>,
}

#[derive(Deserialize)]
struct CommitSigner {
    login: String,
}

/// One account, only when the forge gave both halves of the join: the address the commit
/// carries and the account it belongs to.
fn account_of(identity: Option<CommitIdentity>, account: Option<CommitAccount>) -> Option<ForgeAccount> {
    let email = identity?.email?;
    let account = account?;

    Some(ForgeAccount {
        email,
        login: account.login,
        avatar_url: account.avatar_url,
    })
}

#[derive(Deserialize)]
struct GitlabSignature {
    verification_status: String,
    gpg_key_user_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(remote: &str, kind: ForgeKind) -> Repository {
        Repository::from_remote(&RemoteUrl::new(remote).unwrap(), kind).unwrap()
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
    #[test]
    fn normalizes_provider_statuses_without_losing_failed_outcomes() {
        assert_eq!(check_status("pending"), CheckStatus::Queued);
        assert_eq!(check_status("running"), CheckStatus::InProgress);
        assert_eq!(check_status("failed"), CheckStatus::Completed);
        assert_eq!(check_conclusion("failed"), Some(CheckConclusion::Failure));
        assert_eq!(check_conclusion("canceled"), Some(CheckConclusion::Cancelled));
        assert_eq!(check_conclusion("running"), None);
    }

}
