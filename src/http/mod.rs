pub mod avatar;

use std::path::PathBuf;
use std::sync::Arc;

use jiff::Timestamp;

use poem::http::StatusCode;
use poem::{EndpointExt, Route, middleware::Tracing};
use poem_openapi::{
    ApiResponse, Enum, Object, OpenApi, OpenApiService, param::Path, payload::Json,
};

use crate::analysis::{
    RunStatus, ci_checks, hygiene, links, lockfile, manifest, repository_controls, runner, secret,
    workflow,
};
use crate::auth::{CurrentUser, SessionUser};
use crate::discovery;
use crate::finding::{Ecosystem, Finding, Location, Severity, VersionMovement};
use crate::forge::{ChangeState, ForgeKind};
use crate::id::Id;
use crate::person::{Person, PersonRole};
use crate::signal::{Signal, SignalKey};
use crate::store::{ProjectMemberChange, Store, UserRoleChange};
use crate::user::{ProjectMember, ProjectRole, User, UserRole};
use crate::vcs::mirror::Mirror;
use crate::vcs::{CommitSha, RemoteUrl};
use crate::watch::{Project, SubjectKind};

const TITLE: &str = "error.menu";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const MOUNT: &str = "/api";
const JOB_PAGE: i64 = 50;

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct Health {
    status: String,
    version: String,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct Error {
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum UserRoleOutput {
    Guest,
    Member,
    Admin,
}

impl UserRoleOutput {
    fn from_role(role: UserRole) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum ProjectRoleOutput {
    Viewer,
    Operator,
    Owner,
}

impl ProjectRoleOutput {
    fn from_role(role: ProjectRole) -> Self {
        match role {
            ProjectRole::Viewer => Self::Viewer,
            ProjectRole::Operator => Self::Operator,
            ProjectRole::Owner => Self::Owner,
        }
    }

    fn into_role(self) -> ProjectRole {
        match self {
            Self::Viewer => ProjectRole::Viewer,
            Self::Operator => ProjectRole::Operator,
            Self::Owner => ProjectRole::Owner,
        }
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct UserOutput {
    user_id: String,
    display_name: String,
    role: UserRoleOutput,
}

#[derive(Debug, Object)]
struct UsersOutput {
    users: Vec<UserOutput>,
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

#[derive(Debug, Object)]
struct SetUserRole {
    role: UserRoleOutput,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ProjectMemberOutput {
    user_id: String,
    display_name: String,
    role: ProjectRoleOutput,
}

#[derive(Debug, Object)]
struct ProjectMembersOutput {
    members: Vec<ProjectMemberOutput>,
}

#[derive(Debug, Object)]
struct SetProjectMember {
    role: ProjectRoleOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum Analyzer {
    LockfileDelta,
    ManifestDelta,
    SecretScan,
    LinkInventory,
    WorkflowSecurity,
    RepositoryControls,
    CiCheckRuns,
    RepositoryHygiene,
}

impl Analyzer {
    fn as_str(self) -> &'static str {
        match self {
            Self::LockfileDelta => lockfile::ANALYZER,
            Self::ManifestDelta => manifest::ANALYZER,
            Self::SecretScan => secret::ANALYZER,
            Self::LinkInventory => links::ANALYZER,
            Self::WorkflowSecurity => workflow::ANALYZER,
            Self::RepositoryControls => repository_controls::ANALYZER,
            Self::CiCheckRuns => ci_checks::ANALYZER,
            Self::RepositoryHygiene => hygiene::ANALYZER,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            lockfile::ANALYZER => Some(Self::LockfileDelta),
            manifest::ANALYZER => Some(Self::ManifestDelta),
            secret::ANALYZER => Some(Self::SecretScan),
            links::ANALYZER => Some(Self::LinkInventory),
            workflow::ANALYZER => Some(Self::WorkflowSecurity),
            repository_controls::ANALYZER => Some(Self::RepositoryControls),
            ci_checks::ANALYZER => Some(Self::CiCheckRuns),
            hygiene::ANALYZER => Some(Self::RepositoryHygiene),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum Forge {
    Auto,
    Github,
    Gitlab,
    Gitea,
    Forgejo,
}

impl Forge {
    fn into_kind(self) -> ForgeKind {
        match self {
            Self::Auto => ForgeKind::Auto,
            Self::Github => ForgeKind::Github,
            Self::Gitlab => ForgeKind::Gitlab,
            Self::Gitea => ForgeKind::Gitea,
            Self::Forgejo => ForgeKind::Forgejo,
        }
    }

    fn from_kind(kind: ForgeKind) -> Self {
        match kind {
            ForgeKind::Auto => Self::Auto,
            ForgeKind::Github => Self::Github,
            ForgeKind::Gitlab => Self::Gitlab,
            ForgeKind::Gitea => Self::Gitea,
            ForgeKind::Forgejo => Self::Forgejo,
        }
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct CreateProject {
    name: String,
    remote_url: String,
    uses_default_analyzers: bool,
    analyzers: Vec<Analyzer>,
    forge: Option<Forge>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SetProjectAnalyzers {
    uses_default_analyzers: bool,
    analyzers: Vec<Analyzer>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ProjectOutput {
    project_id: String,
    name: String,
    remote_url: String,
    uses_default_analyzers: bool,
    viewer_role: ProjectRoleOutput,
    analyzers: Vec<Analyzer>,
    forge: Forge,
    description: Option<String>,
    icon_light_path: Option<String>,
    icon_dark_path: Option<String>,
    /// Where the mark lives in the repository, so it can be shown and changed by hand.
    icon_light_source: Option<String>,
    icon_dark_source: Option<String>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ProjectsOutput {
    projects: Vec<ProjectOutput>,
}

/// A file or directory in the repository. `is_image` says whether it could be shown as a
/// mark, so the browser can grey out the rest without fetching anything.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct TreeEntryOutput {
    name: String,
    path: String,
    is_directory: bool,
    is_image: bool,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct TreeOutput {
    path: String,
    entries: Vec<TreeEntryOutput>,
}

#[derive(ApiResponse)]
enum TreeResponse {
    #[oai(status = 200)]
    Found(Json<TreeOutput>),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum SchemeOutput {
    Light,
    Dark,
    Either,
}

/// One image in the repository that could be the project's mark. `scheme` is what the file
/// name says it is drawn for, and `score` is only a ranking, not a measurement.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct IconCandidateOutput {
    path: String,
    scheme: SchemeOutput,
    score: i32,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct IconCandidatesOutput {
    candidates: Vec<IconCandidateOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SetProjectIcon {
    light_path: Option<String>,
    dark_path: Option<String>,
}

#[derive(ApiResponse)]
enum IconCandidatesResponse {
    #[oai(status = 200)]
    Found(Json<IconCandidatesOutput>),
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

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct DescribeProject {
    description: Option<String>,
}

/// A base is optional: a single commit compares against its own first parent, which is what
/// a reader means when they paste one commit link.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct AnalyzeProject {
    base_sha: Option<String>,
    head_sha: String,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct AnalyzerRunOutput {
    analyzer: String,
    status: String,
    detail: Option<String>,
    finding_count: u64,
    signal_count: u64,
    findings: Vec<FindingOutput>,
    signals: Vec<SignalOutput>,
}

/// Which way a dependency moved, when the analyzer could tell. A direction is only claimed
/// for ordered versions, so a flake.lock pinning one commit over another reports `changed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum MovementOutput {
    Added,
    Removed,
    Upgraded,
    Downgraded,
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum EcosystemOutput {
    Cargo,
    Npm,
    Nix,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct PackageOutput {
    ecosystem: EcosystemOutput,
    name: String,
    version: String,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct FindingOutput {
    path: String,
    line_start: Option<u32>,
    line_end: Option<u32>,
    severity: String,
    title: String,
    detail: String,
    package: Option<PackageOutput>,
    movement: Option<MovementOutput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum SignalValueKindOutput {
    Score,
    Flag,
    Count,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SignalOutput {
    key: String,
    value_kind: SignalValueKindOutput,
    score: Option<f64>,
    flag: Option<bool>,
    count: Option<u64>,
    confidence: f64,
    reason: String,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct CheckRunOutput {
    name: String,
    status: String,
    conclusion: Option<String>,
    url: Option<String>,
    log_excerpt_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum SubjectKindOutput {
    Change,
    Branch,
    Commit,
}

/// The identity of what was analysed. `key` is the change number, the branch name, or
/// the commit sha, according to `kind`.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SubjectOutput {
    kind: SubjectKindOutput,
    key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum ChangeStateOutput {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ForgeOutput {
    title: Option<String>,
    author: Option<String>,
    url: Option<String>,
    base_ref: Option<String>,
    head_ref: Option<String>,
    state: Option<ChangeStateOutput>,
    merge_commit_sha: Option<String>,
}

/// One word for the whole analysis, so that every reader ranks severity the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum AnalysisStatus {
    /// An analyzer could not finish, or an analyzer reported a High or Critical finding.
    Alarming,
    /// An analyzer reported a Medium finding and nothing worse.
    Attention,
    /// Every analyzer finished and reported nothing above Low.
    Clear,
    /// No analyzer has run against this change. A closed or merged change is recorded for
    /// its history rather than scanned, so it never earns a verdict.
    Unscanned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum PersonRoleOutput {
    Author,
    Committer,
    CoAuthor,
    SignedOffBy,
    Submitter,
    Reviewer,
}

/// A person a change names. Git writes every one of these from the client, so this is a
/// claim rather than a proven identity.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct PersonOutput {
    identity: String,
    role: PersonRoleOutput,
    label: String,
    name: Option<String>,
    email: Option<String>,
    login: Option<String>,
    avatar_path: String,
}

/// What the commit carries and what the forge says about it. error.menu never checks the
/// cryptography, so `verified` repeats a forge verdict and is absent when none was given.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SignatureOutput {
    present: bool,
    verified: Option<bool>,
    signer: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct AnalysisOutput {
    signature: SignatureOutput,
    snapshot_id: String,
    subject: SubjectOutput,
    status: AnalysisStatus,
    base_sha: Option<String>,
    head_sha: String,
    merge_base_sha: Option<String>,
    forge: ForgeOutput,
    people: Vec<PersonOutput>,
    check_runs: Vec<CheckRunOutput>,
    analyzers: Vec<AnalyzerRunOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct AnalysesOutput {
    analyses: Vec<AnalysisOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct DiscoveryOutput {
    default_branch: AnalysisOutput,
    pull_requests: Vec<AnalysisOutput>,
}

/// What the queue is doing, for a reader who wants to know that scanning happens without
/// them. A job carries its own error, because "nothing has run" and "it tried and the forge
/// refused" are different answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum JobKindOutput {
    Discover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
enum JobStateOutput {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct JobOutput {
    job_id: String,
    project_id: String,
    kind: JobKindOutput,
    state: JobStateOutput,
    attempts: u32,
    last_error: Option<String>,
    available_at: String,
    created_at: String,
    finished_at: Option<String>,
}

#[derive(Debug, Object)]
struct JobsOutput {
    jobs: Vec<JobOutput>,
}

#[derive(ApiResponse)]
enum ListJobsResponse {
    #[oai(status = 200)]
    Found(Json<JobsOutput>),
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

#[derive(ApiResponse)]
enum ListProjectsResponse {
    #[oai(status = 200)]
    Found(Json<ProjectsOutput>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}

#[derive(ApiResponse)]
enum CreateProjectResponse {
    #[oai(status = 201)]
    Created(Json<ProjectOutput>),
    #[oai(status = 400)]
    Invalid(Json<Error>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 403)]
    Forbidden(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}

#[derive(ApiResponse)]
enum GetProjectResponse {
    #[oai(status = 200)]
    Found(Json<ProjectOutput>),
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

#[derive(ApiResponse)]
enum ListAnalysesResponse {
    #[oai(status = 200)]
    Found(Json<AnalysesOutput>),
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

#[derive(ApiResponse)]
enum AnalyzeProjectResponse {
    #[oai(status = 201)]
    Created(Json<AnalysisOutput>),
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

#[derive(ApiResponse)]
enum DiscoverProjectResponse {
    #[oai(status = 200)]
    Found(Json<DiscoveryOutput>),
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

#[derive(ApiResponse)]
enum ProjectMembersResponse {
    #[oai(status = 200)]
    Found(Json<ProjectMembersOutput>),
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

struct Api {
    store: Arc<Store>,
    mirror_root: PathBuf,
}

#[OpenApi]
impl Api {
    #[oai(path = "/health", method = "get")]
    async fn health(&self) -> Json<Health> {
        Json(Health {
            status: "ok".to_owned(),
            version: VERSION.to_owned(),
        })
    }

    #[oai(path = "/tokens", method = "get")]
    async fn list_api_tokens(&self, SessionUser(user): SessionUser) -> ApiTokensResponse {
        match self.store.list_api_tokens(user.id, Timestamp::now()).await {
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
                ApiTokensResponse::Failed(Json(api_token_store_error("list_api_tokens", error)))
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
        if name.is_empty() || name.len() > 128 {
            return ApiTokensResponse::Invalid(Json(Error {
                message: "token name must contain 1 to 128 bytes".to_owned(),
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
        let token = format!("em_pat_{}", crate::auth::random_token());
        let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        match self
            .store
            .create_api_token(user.id, &token_hash, name, expires_at)
            .await
        {
            Ok(stored) => ApiTokensResponse::Created(Json(CreatedApiTokenOutput {
                name: stored.name,
                token,
                created_at: stored.created_at.to_string(),
                expires_at: stored.expires_at.map(|expires_at| expires_at.to_string()),
            })),
            Err(error) => {
                ApiTokensResponse::Failed(Json(api_token_store_error("create_api_token", error)))
            }
        }
    }

    #[oai(path = "/tokens/:name", method = "delete")]
    async fn revoke_api_token(
        &self,
        SessionUser(user): SessionUser,
        name: Path<String>,
    ) -> RevokeApiTokenResponse {
        match self.store.revoke_api_token(user.id, &name.0).await {
            Ok(true) => RevokeApiTokenResponse::Revoked,
            Ok(false) => RevokeApiTokenResponse::Missing(Json(Error {
                message: "API token not found".to_owned(),
            })),
            Err(error) => RevokeApiTokenResponse::Failed(Json(api_token_store_error(
                "revoke_api_token",
                error,
            ))),
        }
    }

    #[oai(path = "/user", method = "get")]
    async fn current_user(&self, CurrentUser(user): CurrentUser) -> UserResponse {
        UserResponse::Found(Json(user_output(user)))
    }

    #[oai(path = "/users", method = "get")]
    async fn list_users(&self, CurrentUser(user): CurrentUser) -> ListUsersResponse {
        if user.role == UserRole::Guest {
            return ListUsersResponse::Forbidden(Json(forbidden()));
        }

        match self.store.list_users().await {
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

        match self.store.user(user_id).await {
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

        match self
            .store
            .set_user_role(user_id, input.0.role.into_role())
            .await
        {
            Ok(UserRoleChange::Updated) => match self.store.user(user_id).await {
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

    #[oai(path = "/projects", method = "get")]
    async fn list_projects(&self, CurrentUser(user): CurrentUser) -> ListProjectsResponse {
        let projects = match self.store.list_projects_for(&user).await {
            Ok(projects) => projects,
            Err(error) => {
                return ListProjectsResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mut output = Vec::with_capacity(projects.len());
        for project in projects {
            let role = match self.store.project_role_for(&user, project.id).await {
                Ok(Some(role)) => role,
                Ok(None) => continue,
                Err(error) => {
                    return ListProjectsResponse::Failed(Json(Error {
                        message: error.to_string(),
                    }));
                }
            };
            match project_output(&self.store, project, role).await {
                Ok(project) => output.push(project),
                Err(error) => {
                    return ListProjectsResponse::Failed(Json(Error {
                        message: error.to_string(),
                    }));
                }
            }
        }

        ListProjectsResponse::Found(Json(ProjectsOutput { projects: output }))
    }

    /// What the queue holds, newest first. The schedule runs without anybody watching, so
    /// this is how a reader finds out that it did, or that it tried and failed.
    #[oai(path = "/jobs", method = "get")]
    async fn jobs(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: poem_openapi::param::Query<Option<String>>,
    ) -> ListJobsResponse {
        let project_id = match project_id
            .0
            .as_deref()
            .map(str::parse::<Id<Project>>)
            .transpose()
        {
            Ok(project_id) => project_id,
            Err(error) => {
                return ListJobsResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_id {
            Some(project_id) => {
                match project_access(&self.store, &user, project_id, ProjectPermission::Viewer)
                    .await
                {
                    ProjectAccess::Allowed { .. } => {}
                    ProjectAccess::Forbidden => {
                        return ListJobsResponse::Forbidden(Json(forbidden()));
                    }
                    ProjectAccess::Missing => {
                        return ListJobsResponse::Missing(Json(missing_project()));
                    }
                    ProjectAccess::Failed(message) => {
                        return ListJobsResponse::Failed(Json(Error { message }));
                    }
                }
            }
            None if user.role != UserRole::Admin => {
                return ListJobsResponse::Forbidden(Json(forbidden()));
            }
            None => {}
        }

        match self.store.recent_jobs(project_id, JOB_PAGE).await {
            Ok(jobs) => ListJobsResponse::Found(Json(JobsOutput {
                jobs: jobs.into_iter().map(job_output).collect(),
            })),
            Err(error) => ListJobsResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects", method = "post")]
    async fn create_project(
        &self,
        CurrentUser(user): CurrentUser,
        input: Json<CreateProject>,
    ) -> CreateProjectResponse {
        if !can_create_project(&user) {
            return CreateProjectResponse::Forbidden(Json(forbidden()));
        }
        let input = input.0;
        let remote = match RemoteUrl::new(&input.remote_url) {
            Ok(remote) => remote,
            Err(error) => {
                return CreateProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        if input.name.trim().is_empty() {
            return CreateProjectResponse::Invalid(Json(Error {
                message: "project name is empty".to_owned(),
            }));
        }

        let analyzers = input
            .analyzers
            .iter()
            .copied()
            .map(Analyzer::as_str)
            .collect::<Vec<_>>();
        let project = match self
            .store
            .create_project(
                user.id,
                input.name.trim(),
                remote,
                input.forge.unwrap_or(Forge::Auto).into_kind(),
                input.uses_default_analyzers,
                &analyzers,
            )
            .await
        {
            Ok(project) => project,
            Err(error) => return failed(error),
        };

        match project_output(&self.store, project, ProjectRole::Owner).await {
            Ok(project) => CreateProjectResponse::Created(Json(project)),
            Err(error) => failed(error),
        }
    }

    #[oai(path = "/projects/:project_id/analyzers", method = "put")]
    async fn set_project_analyzers(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<SetProjectAnalyzers>,
    ) -> GetProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => return GetProjectResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => return GetProjectResponse::Missing(Json(missing_project())),
            ProjectAccess::Failed(message) => {
                return GetProjectResponse::Failed(Json(Error { message }));
            }
        }
        let input = input.0;
        let analyzers = input
            .analyzers
            .iter()
            .copied()
            .map(Analyzer::as_str)
            .collect::<Vec<_>>();
        if let Err(error) = self
            .store
            .set_project_analyzers(project_id, input.uses_default_analyzers, &analyzers)
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }

        match self.store.project(project_id).await {
            Ok(Some(project)) => {
                match project_output(&self.store, project, ProjectRole::Owner).await {
                    Ok(output) => GetProjectResponse::Found(Json(output)),
                    Err(error) => GetProjectResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
            Ok(None) => GetProjectResponse::Missing(Json(missing_project())),
            Err(error) => GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// Puts one recorded change through the analyzers. Discovery records a closed or merged
    /// change without scanning it, and this is how a reader asks for that work anyway.
    #[oai(path = "/projects/:project_id/changes/:number/scan", method = "post")]
    async fn scan_change(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        number: Path<u64>,
    ) -> AnalyzeProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return AnalyzeProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Operator).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return AnalyzeProjectResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return AnalyzeProjectResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return AnalyzeProjectResponse::Failed(Json(Error { message }));
            }
        }
        let snapshot = match discovery::scan_change(
            &self.store,
            &self.mirror_root,
            project_id,
            number.0,
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(discovery::DiscoveryError::ProjectNotFound)
            | Err(discovery::DiscoveryError::ChangeNotFound) => {
                return AnalyzeProjectResponse::Missing(Json(Error {
                    message: "that change has not been discovered yet".to_owned(),
                }));
            }
            Err(error) => {
                return AnalyzeProjectResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };

        match self.store.analyses_for_project(project_id).await {
            Ok(mut analyses) => match take_analysis(&mut analyses, snapshot.id) {
                Ok(output) => AnalyzeProjectResponse::Created(Json(output)),
                Err(message) => AnalyzeProjectResponse::Failed(Json(Error { message })),
            },
            Err(error) => AnalyzeProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// One directory of the repository at the default branch, for browsing it. Directories
    /// come first, so the list reads the way a file manager does.
    #[oai(path = "/projects/:project_id/tree", method = "get")]
    async fn project_tree(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        path: poem_openapi::param::Query<Option<String>>,
    ) -> TreeResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return TreeResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project =
            match project_access(&self.store, &user, project_id, ProjectPermission::Viewer).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => return TreeResponse::Forbidden(Json(forbidden())),
                ProjectAccess::Missing => return TreeResponse::Missing(Json(missing_project())),
                ProjectAccess::Failed(message) => {
                    return TreeResponse::Failed(Json(Error { message }));
                }
            };
        let head = match self.store.default_branch_head(project_id).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                return TreeResponse::Missing(Json(Error {
                    message: "run discovery first, so the default branch is known".to_owned(),
                }));
            }
            Err(error) => {
                return TreeResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mirror = match Mirror::open(&self.mirror_root, &project.remote).await {
            Ok(mirror) => mirror,
            Err(error) => {
                return TreeResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let directory = path.0.unwrap_or_default();
        match mirror.entries_at(&head, &directory).await {
            Ok(entries) => TreeResponse::Found(Json(TreeOutput {
                path: directory,
                entries: entries.into_iter().map(entry_output).collect(),
            })),
            Err(error) => TreeResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// Every image in the repository that could be the project's mark, best first. The
    /// ranking is a guess, so the whole list is offered and the reader decides.
    #[oai(path = "/projects/:project_id/icon-candidates", method = "get")]
    async fn icon_candidates(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> IconCandidatesResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return IconCandidatesResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project =
            match project_access(&self.store, &user, project_id, ProjectPermission::Viewer).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => {
                    return IconCandidatesResponse::Forbidden(Json(forbidden()));
                }
                ProjectAccess::Missing => {
                    return IconCandidatesResponse::Missing(Json(missing_project()));
                }
                ProjectAccess::Failed(message) => {
                    return IconCandidatesResponse::Failed(Json(Error { message }));
                }
            };
        let head = match self.store.default_branch_head(project_id).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                return IconCandidatesResponse::Missing(Json(Error {
                    message: "run discovery first, so the default branch is known".to_owned(),
                }));
            }
            Err(error) => {
                return IconCandidatesResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mirror = match Mirror::open(&self.mirror_root, &project.remote).await {
            Ok(mirror) => mirror,
            Err(error) => {
                return IconCandidatesResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match crate::icon::suggest(&mirror, &head, &project.name).await {
            Ok(candidates) => IconCandidatesResponse::Found(Json(IconCandidatesOutput {
                candidates: candidates.into_iter().map(candidate_output).collect(),
            })),
            Err(error) => IconCandidatesResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    /// Sets the mark by hand. A path is kept rather than the bytes, so the picture follows
    /// the default branch instead of going stale.
    #[oai(path = "/projects/:project_id/icon", method = "put")]
    async fn set_project_icon(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<SetProjectIcon>,
    ) -> GetProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project = match project_access(&self.store, &user, project_id, ProjectPermission::Owner)
            .await
        {
            ProjectAccess::Allowed { project, .. } => project,
            ProjectAccess::Forbidden => return GetProjectResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => return GetProjectResponse::Missing(Json(missing_project())),
            ProjectAccess::Failed(message) => {
                return GetProjectResponse::Failed(Json(Error { message }));
            }
        };
        let icon = match icon_from_input(&input.0) {
            Ok(icon) => icon,
            Err(message) => return GetProjectResponse::Invalid(Json(Error { message })),
        };
        if let Err(error) = self
            .store
            .describe_project(project_id, project.description.as_deref(), &icon)
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }
        project_response(&self.store, project_id, ProjectRole::Owner).await
    }

    /// A description is written by hand today. The intent is for a reviewing model to keep
    /// it current, so it is stored on the project rather than derived at read time.
    #[oai(path = "/projects/:project_id/description", method = "put")]
    async fn describe_project(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<DescribeProject>,
    ) -> GetProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project = match project_access(&self.store, &user, project_id, ProjectPermission::Owner)
            .await
        {
            ProjectAccess::Allowed { project, .. } => project,
            ProjectAccess::Forbidden => return GetProjectResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => return GetProjectResponse::Missing(Json(missing_project())),
            ProjectAccess::Failed(message) => {
                return GetProjectResponse::Failed(Json(Error { message }));
            }
        };
        let description = input.0.description.filter(|text| !text.trim().is_empty());
        if let Err(error) = self
            .store
            .describe_project(project_id, description.as_deref(), &project.icon)
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }
        project_response(&self.store, project_id, ProjectRole::Owner).await
    }

    #[oai(path = "/projects/:project_id", method = "get")]
    async fn project(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> GetProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { project, role } => {
                match project_output(&self.store, project, role).await {
                    Ok(project) => GetProjectResponse::Found(Json(project)),
                    Err(error) => GetProjectResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
            ProjectAccess::Forbidden => GetProjectResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => GetProjectResponse::Missing(Json(missing_project())),
            ProjectAccess::Failed(message) => GetProjectResponse::Failed(Json(Error { message })),
        }
    }

    #[oai(path = "/projects/:project_id/members", method = "get")]
    async fn list_project_members(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> ProjectMembersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectMembersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectMembersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectMembersResponse::Failed(Json(Error { message }));
            }
        }
        match self.store.list_project_members(project_id).await {
            Ok(members) => ProjectMembersResponse::Found(Json(ProjectMembersOutput {
                members: members.into_iter().map(project_member_output).collect(),
            })),
            Err(error) => ProjectMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id/members/:user_id", method = "put")]
    async fn set_project_member(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        user_id: Path<String>,
        input: Json<SetProjectMember>,
    ) -> ProjectMembersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectMembersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectMembersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectMembersResponse::Failed(Json(Error { message }));
            }
        }
        let user_id = match user_id.0.parse::<Id<User>>() {
            Ok(user_id) => user_id,
            Err(_) => return ProjectMembersResponse::Missing(Json(missing_user())),
        };
        match self.store.user(user_id).await {
            Ok(Some(member)) if member.role != UserRole::Guest => {}
            Ok(Some(_)) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: "a guest must become a member before joining a project".to_owned(),
                }));
            }
            Ok(None) => return ProjectMembersResponse::Missing(Json(missing_user())),
            Err(error) => {
                return ProjectMembersResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        }
        match self
            .store
            .set_project_member(project_id, user_id, input.0.role.into_role())
            .await
        {
            Ok(ProjectMemberChange::Updated) => {
                match self.store.list_project_members(project_id).await {
                    Ok(members) => ProjectMembersResponse::Found(Json(ProjectMembersOutput {
                        members: members.into_iter().map(project_member_output).collect(),
                    })),
                    Err(error) => ProjectMembersResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
            Ok(ProjectMemberChange::FinalOwner) => ProjectMembersResponse::Invalid(Json(Error {
                message: "a project must keep an owner".to_owned(),
            })),
            Ok(ProjectMemberChange::Missing | ProjectMemberChange::Removed) => {
                ProjectMembersResponse::Missing(Json(missing_user()))
            }
            Err(error) => ProjectMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id/members/:user_id", method = "delete")]
    async fn remove_project_member(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        user_id: Path<String>,
    ) -> ProjectMembersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectMembersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectMembersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectMembersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectMembersResponse::Failed(Json(Error { message }));
            }
        }
        let user_id = match user_id.0.parse::<Id<User>>() {
            Ok(user_id) => user_id,
            Err(_) => return ProjectMembersResponse::Missing(Json(missing_user())),
        };
        match self.store.remove_project_member(project_id, user_id).await {
            Ok(ProjectMemberChange::Removed) => {
                match self.store.list_project_members(project_id).await {
                    Ok(members) => ProjectMembersResponse::Found(Json(ProjectMembersOutput {
                        members: members.into_iter().map(project_member_output).collect(),
                    })),
                    Err(error) => ProjectMembersResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
            Ok(ProjectMemberChange::Missing) => {
                ProjectMembersResponse::Missing(Json(missing_user()))
            }
            Ok(ProjectMemberChange::FinalOwner) => ProjectMembersResponse::Invalid(Json(Error {
                message: "a project must keep an owner".to_owned(),
            })),
            Ok(ProjectMemberChange::Updated) => ProjectMembersResponse::Failed(Json(Error {
                message: "project membership state was invalid".to_owned(),
            })),
            Err(error) => ProjectMembersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id/analyses", method = "get")]
    async fn analyses(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> ListAnalysesResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ListAnalysesResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => return ListAnalysesResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => {
                return ListAnalysesResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ListAnalysesResponse::Failed(Json(Error { message }));
            }
        }
        match self.store.analyses_for_project(project_id).await {
            Ok(analyses) => ListAnalysesResponse::Found(Json(AnalysesOutput {
                analyses: analyses.into_iter().map(analysis_output).collect(),
            })),
            Err(error) => ListAnalysesResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id/discover", method = "post")]
    async fn discover_project(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> DiscoverProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return DiscoverProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Operator).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return DiscoverProjectResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return DiscoverProjectResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return DiscoverProjectResponse::Failed(Json(Error { message }));
            }
        }
        let discovery = match discovery::run(&self.store, &self.mirror_root, project_id).await {
            Ok(discovery) => discovery,
            Err(discovery::DiscoveryError::ProjectNotFound) => {
                return DiscoverProjectResponse::Missing(Json(missing_project()));
            }
            Err(error) => {
                return DiscoverProjectResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match discovery_output(&self.store, project_id, discovery).await {
            Ok(output) => DiscoverProjectResponse::Found(Json(output)),
            Err(message) => DiscoverProjectResponse::Failed(Json(Error { message })),
        }
    }

    #[oai(path = "/projects/:project_id/analyses", method = "post")]
    async fn analyze_project(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<AnalyzeProject>,
    ) -> AnalyzeProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return AnalyzeProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.store, &user, project_id, ProjectPermission::Operator).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return AnalyzeProjectResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return AnalyzeProjectResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return AnalyzeProjectResponse::Failed(Json(Error { message }));
            }
        }
        let base = match input.0.base_sha.as_deref().map(CommitSha::new).transpose() {
            Ok(base) => base,
            Err(error) => {
                return AnalyzeProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let head = match CommitSha::new(&input.0.head_sha) {
            Ok(head) => head,
            Err(error) => {
                return AnalyzeProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match runner::run(&self.store, &self.mirror_root, project_id, base, head).await {
            Ok(analysis) => match self.store.analyses_for_project(project_id).await {
                Ok(analyses) => match analyses
                    .into_iter()
                    .find(|stored| stored.snapshot.id == analysis.snapshot.id)
                {
                    Some(stored) => AnalyzeProjectResponse::Created(Json(analysis_output(stored))),
                    None => AnalyzeProjectResponse::Failed(Json(Error {
                        message: "analysis was not stored".to_owned(),
                    })),
                },
                Err(error) => AnalyzeProjectResponse::Failed(Json(Error {
                    message: error.to_string(),
                })),
            },
            Err(runner::AnalysisError::ProjectNotFound) => {
                AnalyzeProjectResponse::Missing(Json(missing_project()))
            }
            Err(runner::AnalysisError::RootCommit) => {
                AnalyzeProjectResponse::Invalid(Json(Error {
                    message: "that commit has no parent, so give a base commit to compare against"
                        .to_owned(),
                }))
            }
            Err(error) => AnalyzeProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

enum ProjectPermission {
    Viewer,
    Operator,
    Owner,
}

enum ProjectAccess {
    Allowed { project: Project, role: ProjectRole },
    Forbidden,
    Missing,
    Failed(String),
}

async fn project_access(
    store: &Store,
    user: &User,
    project_id: Id<Project>,
    permission: ProjectPermission,
) -> ProjectAccess {
    let project = match store.project(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return ProjectAccess::Missing,
        Err(error) => return ProjectAccess::Failed(error.to_string()),
    };
    let role = match store.project_role_for(user, project_id).await {
        Ok(role) => role,
        Err(error) => return ProjectAccess::Failed(error.to_string()),
    };
    let allowed = match (role, permission) {
        (Some(_), ProjectPermission::Viewer)
        | (Some(ProjectRole::Operator | ProjectRole::Owner), ProjectPermission::Operator)
        | (Some(ProjectRole::Owner), ProjectPermission::Owner) => true,
        _ => false,
    };
    if let Some(role) = role.filter(|_| allowed) {
        ProjectAccess::Allowed { project, role }
    } else {
        ProjectAccess::Forbidden
    }
}

fn can_create_project(user: &User) -> bool {
    matches!(user.role, UserRole::Member | UserRole::Admin)
}

fn user_output(user: User) -> UserOutput {
    UserOutput {
        user_id: user.id.encode(),
        display_name: user.display_name,
        role: UserRoleOutput::from_role(user.role),
    }
}

fn project_member_output(member: ProjectMember) -> ProjectMemberOutput {
    ProjectMemberOutput {
        user_id: member.user_id.encode(),
        display_name: member.display_name,
        role: ProjectRoleOutput::from_role(member.role),
    }
}

fn forbidden() -> Error {
    Error {
        message: "the current user does not have the required role".to_owned(),
    }
}

fn missing_project() -> Error {
    Error {
        message: "project was not found".to_owned(),
    }
}

fn missing_user() -> Error {
    Error {
        message: "user was not found".to_owned(),
    }
}

fn api_token_store_error(operation: &'static str, error: crate::store::StoreError) -> Error {
    crate::auth::log_store_error(operation, &error);
    Error {
        message: "API token storage operation failed".to_owned(),
    }
}

async fn project_response(
    store: &Store,
    project_id: Id<Project>,
    role: ProjectRole,
) -> GetProjectResponse {
    match store.project(project_id).await {
        Ok(Some(project)) => match project_output(store, project, role).await {
            Ok(project) => GetProjectResponse::Found(Json(project)),
            Err(error) => GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        },
        Ok(None) => GetProjectResponse::Missing(Json(missing_project())),
        Err(error) => GetProjectResponse::Failed(Json(Error {
            message: error.to_string(),
        })),
    }
}

async fn discovery_output(
    store: &Store,
    project_id: Id<Project>,
    discovery: discovery::Discovery,
) -> Result<DiscoveryOutput, String> {
    let mut analyses = store
        .analyses_for_project(project_id)
        .await
        .map_err(|error| error.to_string())?;
    let default_branch =
        take_analysis(&mut analyses, discovery.default_branch.analysis.snapshot.id)?;
    let mut pull_requests = Vec::with_capacity(discovery.changes.len());
    for change in discovery.changes {
        pull_requests.push(take_analysis(&mut analyses, change.snapshot.id)?);
    }

    Ok(DiscoveryOutput {
        default_branch,
        pull_requests,
    })
}

fn take_analysis(
    analyses: &mut Vec<crate::store::AnalysisRecord>,
    snapshot_id: Id<crate::watch::Snapshot>,
) -> Result<AnalysisOutput, String> {
    let Some(index) = analyses
        .iter()
        .position(|analysis| analysis.snapshot.id == snapshot_id)
    else {
        return Err("discovery analysis was not stored".to_owned());
    };

    Ok(analysis_output(analyses.remove(index)))
}

fn failed(error: crate::store::StoreError) -> CreateProjectResponse {
    CreateProjectResponse::Failed(Json(Error {
        message: error.to_string(),
    }))
}

async fn project_output(
    store: &Store,
    project: Project,
    viewer_role: ProjectRole,
) -> Result<ProjectOutput, crate::store::StoreError> {
    let analyzers = store
        .effective_project_analyzers(&project)
        .await?
        .into_iter()
        .map(|analyzer| {
            Analyzer::from_str(&analyzer).ok_or(crate::store::StoreError::UnknownAnalyzer(analyzer))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProjectOutput {
        project_id: project.id.encode(),
        name: project.name,
        remote_url: project.remote.to_string(),
        uses_default_analyzers: project.uses_default_analyzers,
        viewer_role: ProjectRoleOutput::from_role(viewer_role),
        analyzers,
        forge: Forge::from_kind(project.forge_kind),
        description: project.description,
        icon_light_path: project
            .icon
            .light
            .as_ref()
            .map(|path| icon_url(project.id, "light", path)),
        icon_dark_path: project
            .icon
            .dark
            .as_ref()
            .map(|path| icon_url(project.id, "dark", path)),
        icon_light_source: project
            .icon
            .light
            .as_ref()
            .map(|path| path.as_str().to_owned()),
        icon_dark_source: project
            .icon
            .dark
            .as_ref()
            .map(|path| path.as_str().to_owned()),
    })
}

fn job_output(job: crate::queue::JobRecord) -> JobOutput {
    JobOutput {
        job_id: job.id.encode(),
        project_id: job.project_id.encode(),
        kind: match job.kind {
            crate::queue::JobKind::Discover => JobKindOutput::Discover,
        },
        state: match job.state {
            crate::queue::JobState::Queued => JobStateOutput::Queued,
            crate::queue::JobState::Running => JobStateOutput::Running,
            crate::queue::JobState::Done => JobStateOutput::Done,
            crate::queue::JobState::Failed => JobStateOutput::Failed,
        },
        attempts: job.attempts.max(0) as u32,
        last_error: job.last_error,
        available_at: job.available_at.to_string(),
        created_at: job.created_at.to_string(),
        finished_at: job.finished_at.map(|at| at.to_string()),
    }
}

fn analysis_output(analysis: crate::store::AnalysisRecord) -> AnalysisOutput {
    let status = analysis_status(&analysis.runs);
    let analyzers = analysis
        .runs
        .into_iter()
        .map(|stored_run| {
            let findings = stored_run
                .findings
                .into_iter()
                .map(finding_output)
                .collect::<Vec<_>>();
            let signals = stored_run
                .signals
                .into_iter()
                .map(signal_output)
                .collect::<Vec<_>>();
            let (status, detail) = run_status_output(stored_run.run.status);

            AnalyzerRunOutput {
                analyzer: stored_run.run.analyzer,
                status,
                detail,
                finding_count: findings.len() as u64,
                signal_count: signals.len() as u64,
                findings,
                signals,
            }
        })
        .collect();
    let check_runs = analysis
        .check_runs
        .into_iter()
        .map(check_run_output)
        .collect();

    AnalysisOutput {
        snapshot_id: analysis.snapshot.id.encode(),
        subject: subject_output(analysis.subject.kind),
        signature: SignatureOutput {
            present: analysis.snapshot.forge.signature.present,
            verified: analysis.snapshot.forge.signature.verified,
            signer: analysis.snapshot.forge.signature.signer.clone(),
            reason: analysis.snapshot.forge.signature.reason.clone(),
        },
        status,
        base_sha: analysis.snapshot.base.map(|sha| sha.to_string()),
        head_sha: analysis.snapshot.head.to_string(),
        merge_base_sha: analysis.snapshot.merge_base.map(|sha| sha.to_string()),
        people: analysis
            .snapshot
            .forge
            .people
            .iter()
            .map(person_output)
            .collect(),
        forge: ForgeOutput {
            title: analysis.snapshot.forge.title,
            author: analysis.snapshot.forge.author,
            url: analysis.snapshot.forge.url,
            base_ref: analysis.snapshot.forge.base_ref,
            head_ref: analysis.snapshot.forge.head_ref,
            state: analysis.snapshot.forge.state.map(change_state_output),
            merge_commit_sha: analysis
                .snapshot
                .forge
                .merge_commit
                .map(|sha| sha.to_string()),
        },
        check_runs,
        analyzers,
    }
}

fn person_output(person: &Person) -> PersonOutput {
    let identity = person.identity();

    PersonOutput {
        avatar_path: format!("{MOUNT}/avatars/{identity}"),
        identity,
        role: person_role_output(person.role),
        label: person.label().to_owned(),
        name: person.name.clone(),
        email: person.email.clone(),
        login: person.login.clone(),
    }
}

fn person_role_output(role: PersonRole) -> PersonRoleOutput {
    match role {
        PersonRole::Author => PersonRoleOutput::Author,
        PersonRole::Committer => PersonRoleOutput::Committer,
        PersonRole::CoAuthor => PersonRoleOutput::CoAuthor,
        PersonRole::SignedOffBy => PersonRoleOutput::SignedOffBy,
        PersonRole::Submitter => PersonRoleOutput::Submitter,
        PersonRole::Reviewer => PersonRoleOutput::Reviewer,
    }
}

fn subject_output(kind: SubjectKind) -> SubjectOutput {
    match kind {
        SubjectKind::Change { number } => SubjectOutput {
            kind: SubjectKindOutput::Change,
            key: number.to_string(),
        },
        SubjectKind::Branch { name } => SubjectOutput {
            kind: SubjectKindOutput::Branch,
            key: name,
        },
        SubjectKind::Commit { sha } => SubjectOutput {
            kind: SubjectKindOutput::Commit,
            key: sha.to_string(),
        },
    }
}

fn change_state_output(state: ChangeState) -> ChangeStateOutput {
    match state {
        ChangeState::Open => ChangeStateOutput::Open,
        ChangeState::Closed => ChangeStateOutput::Closed,
        ChangeState::Merged => ChangeStateOutput::Merged,
    }
}

fn analysis_status(runs: &[crate::store::AnalysisRunRecord]) -> AnalysisStatus {
    if runs.is_empty() {
        return AnalysisStatus::Unscanned;
    }

    let mut status = AnalysisStatus::Clear;

    for stored_run in runs {
        match stored_run.run.status {
            RunStatus::Failed { .. } | RunStatus::TimedOut => return AnalysisStatus::Alarming,
            RunStatus::Running | RunStatus::Succeeded | RunStatus::Skipped { .. } => {}
        }

        for finding in &stored_run.findings {
            match finding.severity {
                Severity::Critical | Severity::High => return AnalysisStatus::Alarming,
                Severity::Medium => status = AnalysisStatus::Attention,
                Severity::Low | Severity::Info => {}
            }
        }
    }

    status
}

fn run_status_output(status: RunStatus) -> (String, Option<String>) {
    match status {
        RunStatus::Running => ("running".to_owned(), None),
        RunStatus::Succeeded => ("succeeded".to_owned(), None),
        RunStatus::Failed { message } => ("failed".to_owned(), Some(message)),
        RunStatus::TimedOut => ("timed_out".to_owned(), None),
        RunStatus::Skipped { reason } => ("skipped".to_owned(), Some(reason)),
    }
}

fn finding_output(finding: Finding) -> FindingOutput {
    let (path, line_start, line_end, package) = match finding.location {
        Location::File { path, span } => (
            path.to_string(),
            span.map(|span| span.start),
            span.map(|span| span.end),
            None,
        ),
        Location::Package {
            path,
            ecosystem,
            name,
            version,
        } => (
            path.to_string(),
            None,
            None,
            Some(PackageOutput {
                ecosystem: ecosystem_output(ecosystem),
                name,
                version,
            }),
        ),
    };

    FindingOutput {
        path,
        line_start,
        line_end,
        severity: severity_output(finding.severity).to_owned(),
        title: finding.title,
        detail: finding.detail,
        package,
        movement: finding.movement.map(movement_output),
    }
}

/// The URL the mark is served from. The path in the repository is folded into it, because
/// the picture at `/icon/light` changes whenever the reader picks a different file, and a
/// URL that never changes would be answered from the browser cache with the old one.
fn icon_url(project_id: Id<Project>, scheme: &str, path: &crate::vcs::RepoPath) -> String {
    let version = &blake3::hash(path.as_str().as_bytes()).to_hex()[..8];

    format!(
        "{MOUNT}/projects/{}/icon/{scheme}?v={version}",
        project_id.encode()
    )
}

fn entry_output(entry: crate::vcs::mirror::TreeEntry) -> TreeEntryOutput {
    TreeEntryOutput {
        is_image: !entry.is_directory && is_image_path(entry.path.as_str()),
        name: entry.name,
        path: entry.path.as_str().to_owned(),
        is_directory: entry.is_directory,
    }
}

fn is_image_path(path: &str) -> bool {
    matches!(
        path.rsplit('.').next().map(str::to_lowercase).as_deref(),
        Some("svg" | "png" | "webp" | "ico" | "jpg" | "jpeg" | "gif")
    )
}

fn candidate_output(candidate: crate::icon::Candidate) -> IconCandidateOutput {
    IconCandidateOutput {
        path: candidate.path.as_str().to_owned(),
        scheme: match candidate.scheme {
            crate::icon::Scheme::Light => SchemeOutput::Light,
            crate::icon::Scheme::Dark => SchemeOutput::Dark,
            crate::icon::Scheme::Either => SchemeOutput::Either,
        },
        score: candidate.score,
    }
}

/// A path arrives from the reader, so it is parsed before it reaches the repository.
fn icon_from_input(input: &SetProjectIcon) -> Result<crate::watch::ProjectIcon, String> {
    let parse = |path: &Option<String>| -> Result<Option<crate::vcs::RepoPath>, String> {
        path.as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(|path| {
                crate::vcs::RepoPath::new(path).map_err(|_| format!("{path} is not a usable path"))
            })
            .transpose()
    };

    Ok(crate::watch::ProjectIcon {
        light: parse(&input.light_path)?,
        dark: parse(&input.dark_path)?,
    })
}

fn movement_output(movement: VersionMovement) -> MovementOutput {
    match movement {
        VersionMovement::Added => MovementOutput::Added,
        VersionMovement::Removed => MovementOutput::Removed,
        VersionMovement::Upgraded => MovementOutput::Upgraded,
        VersionMovement::Downgraded => MovementOutput::Downgraded,
        VersionMovement::Changed => MovementOutput::Changed,
    }
}

fn ecosystem_output(ecosystem: Ecosystem) -> EcosystemOutput {
    match ecosystem {
        Ecosystem::Cargo => EcosystemOutput::Cargo,
        Ecosystem::Npm => EcosystemOutput::Npm,
        Ecosystem::Nix => EcosystemOutput::Nix,
    }
}

fn severity_output(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

fn signal_output(signal: Signal) -> SignalOutput {
    let (value_kind, score, flag, count) = match signal.value {
        crate::signal::SignalValue::Score(score) => (
            SignalValueKindOutput::Score,
            Some(f64::from(score.value())),
            None,
            None,
        ),
        crate::signal::SignalValue::Flag(flag) => {
            (SignalValueKindOutput::Flag, None, Some(flag), None)
        }
        crate::signal::SignalValue::Count(count) => {
            (SignalValueKindOutput::Count, None, None, Some(count))
        }
    };

    SignalOutput {
        key: signal_key_output(signal.key).to_owned(),
        value_kind,
        score,
        flag,
        count,
        confidence: f64::from(signal.confidence.value()),
        reason: signal.reason,
    }
}

fn check_run_output(check_run: ci_checks::CheckRun) -> CheckRunOutput {
    CheckRunOutput {
        name: check_run.name,
        status: match check_run.status {
            ci_checks::CheckStatus::Queued => "queued".to_owned(),
            ci_checks::CheckStatus::InProgress => "in_progress".to_owned(),
            ci_checks::CheckStatus::Completed => "completed".to_owned(),
        },
        conclusion: check_run.conclusion.map(|conclusion| match conclusion {
            ci_checks::CheckConclusion::Success => "success".to_owned(),
            ci_checks::CheckConclusion::Failure => "failure".to_owned(),
            ci_checks::CheckConclusion::Neutral => "neutral".to_owned(),
            ci_checks::CheckConclusion::Skipped => "skipped".to_owned(),
            ci_checks::CheckConclusion::Cancelled => "cancelled".to_owned(),
            ci_checks::CheckConclusion::TimedOut => "timed_out".to_owned(),
            ci_checks::CheckConclusion::ActionRequired => "action_required".to_owned(),
            ci_checks::CheckConclusion::Stale => "stale".to_owned(),
        }),
        url: check_run.url,
        log_excerpt_ref: check_run.log_excerpt_ref,
    }
}

fn signal_key_output(key: SignalKey) -> &'static str {
    match key {
        SignalKey::OffTask => "off_task",
        SignalKey::DiffSize => "diff_size",
        SignalKey::BlastRadius => "blast_radius",
        SignalKey::DependencyRisk => "dependency_risk",
        SignalKey::TestsFailing => "tests_failing",
        SignalKey::LinksAdded => "links_added",
        SignalKey::RepositoryHygiene => "repository_hygiene",
    }
}

fn image_response(request: &poem::Request, content_type: &str, bytes: Vec<u8>) -> poem::Response {
    let etag = format!("\"{}\"", &blake3::hash(&bytes).to_hex()[..16]);
    let known = request
        .headers()
        .get("if-none-match")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag);

    if known {
        return poem::Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header("etag", etag)
            .header("cache-control", "no-cache")
            .header("content-security-policy", "default-src 'none'; sandbox")
            .header("x-content-type-options", "nosniff")
            .finish();
    }

    poem::Response::builder()
        .content_type(content_type)
        .header("etag", etag)
        .header("cache-control", "no-cache")
        .header("content-security-policy", "default-src 'none'; sandbox")
        .header("x-content-type-options", "nosniff")
        .body(bytes)
}

#[poem::handler]
async fn serve_blob(
    poem::web::Path(project_id): poem::web::Path<String>,
    poem::web::Query(query): poem::web::Query<BlobQuery>,
    poem::web::Data(state): poem::web::Data<&IconState>,
    CurrentUser(user): CurrentUser,
    request: &poem::Request,
) -> poem::Response {
    let not_found = || {
        poem::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .finish()
    };
    let Ok(project_id) = project_id.parse::<Id<Project>>() else {
        return not_found();
    };
    let Ok(path) = crate::vcs::RepoPath::new(&query.path) else {
        return not_found();
    };
    if !is_image_path(path.as_str()) {
        return not_found();
    }
    let project =
        match project_access(&state.store, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { project, .. } => project,
            ProjectAccess::Forbidden => {
                return poem::Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .finish();
            }
            ProjectAccess::Missing => return not_found(),
            ProjectAccess::Failed(_) => {
                return poem::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .finish();
            }
        };
    let head = match state.store.default_branch_head(project_id).await {
        Ok(Some(head)) => head,
        Ok(None) => return not_found(),
        Err(_) => {
            return poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish();
        }
    };
    let mirror = match Mirror::open(&state.mirror_root, &project.remote).await {
        Ok(mirror) => mirror,
        Err(_) => {
            return poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish();
        }
    };
    match mirror
        .bytes_at(&head, &path, crate::icon::MAX_ICON_BYTES)
        .await
    {
        Ok(Some(bytes)) => {
            let content_type = icon_content_type(&path);
            if content_type == "image/svg+xml" && !crate::icon::svg_is_safe(&bytes) {
                not_found()
            } else {
                image_response(request, content_type, bytes)
            }
        }
        Ok(None) => not_found(),
        Err(_) => poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .finish(),
    }
}

#[derive(serde::Deserialize)]
struct BlobQuery {
    path: String,
}

#[poem::handler]
async fn serve_icon(
    poem::web::Path((project_id, scheme)): poem::web::Path<(String, String)>,
    poem::web::Data(state): poem::web::Data<&IconState>,
    CurrentUser(user): CurrentUser,
    request: &poem::Request,
) -> poem::Response {
    let not_found = || {
        poem::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .finish()
    };
    let Ok(project_id) = project_id.parse::<Id<Project>>() else {
        return not_found();
    };
    let project =
        match project_access(&state.store, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { project, .. } => project,
            ProjectAccess::Forbidden => {
                return poem::Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .finish();
            }
            ProjectAccess::Missing => return not_found(),
            ProjectAccess::Failed(_) => {
                return poem::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .finish();
            }
        };
    let path = match scheme.as_str() {
        "light" => project.icon.light,
        "dark" => project.icon.dark,
        _ => None,
    };
    let Some(path) = path else {
        return not_found();
    };
    let mirror = match Mirror::open(&state.mirror_root, &project.remote).await {
        Ok(mirror) => mirror,
        Err(_) => {
            return poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish();
        }
    };
    let head = match state.store.default_branch_head(project_id).await {
        Ok(Some(head)) => head,
        Ok(None) => return not_found(),
        Err(_) => {
            return poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .finish();
        }
    };
    match mirror
        .bytes_at(&head, &path, crate::icon::MAX_ICON_BYTES)
        .await
    {
        Ok(Some(bytes)) => {
            let content_type = icon_content_type(&path);
            if content_type == "image/svg+xml" && !crate::icon::svg_is_safe(&bytes) {
                not_found()
            } else {
                image_response(request, content_type, bytes)
            }
        }
        Ok(None) => not_found(),
        Err(_) => poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .finish(),
    }
}

fn icon_content_type(path: &crate::vcs::RepoPath) -> &'static str {
    match path
        .as_str()
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    }
}

#[derive(Clone)]
struct IconState {
    store: Arc<Store>,
    mirror_root: PathBuf,
}

#[poem::handler]
async fn serve_avatar(
    poem::web::Path(identity): poem::web::Path<String>,
    poem::web::Data(state): poem::web::Data<&AvatarState>,
) -> poem::Response {
    match avatar::read(&state.store, &state.root, &identity).await {
        Ok(picture) => poem::Response::builder()
            .content_type(picture.content_type)
            .header(
                "cache-control",
                if picture.cacheable {
                    "public, max-age=86400"
                } else {
                    "no-store"
                },
            )
            .body(picture.bytes),
        Err(avatar::AvatarError::Identity) => poem::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .finish(),
        Err(error) => poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(error.to_string()),
    }
}

#[derive(Clone)]
struct AvatarState {
    store: Arc<Store>,
    root: PathBuf,
}

/// One directory holds everything error.menu writes for a project: the bare mirrors it
/// reads and the avatars it has already fetched.
pub fn routes(
    store: Arc<Store>,
    data_root: PathBuf,
    github_auth: Option<Arc<crate::auth::GithubAuth>>,
) -> Route {
    let avatars = AvatarState {
        store: Arc::clone(&store),
        root: data_root.join("avatars"),
    };
    let blobs = IconState {
        store: Arc::clone(&store),
        mirror_root: data_root.join("mirrors"),
    };
    let icons = IconState {
        store: Arc::clone(&store),
        mirror_root: data_root.join("mirrors"),
    };
    let auth_store = Arc::clone(&store);
    let mcp_store = Arc::clone(&store);
    let mcp_auth_store = Arc::clone(&store);
    let service = OpenApiService::new(
        Api {
            store,
            mirror_root: data_root.join("mirrors"),
        },
        TITLE,
        VERSION,
    )
    .server(MOUNT);
    let specification = service.spec_endpoint();

    let api_routes = Route::new()
        .at("/avatars/:identity", poem::get(serve_avatar).data(avatars))
        .at(
            "/projects/:project_id/icon/:scheme",
            poem::get(serve_icon).data(icons),
        )
        .at(
            "/projects/:project_id/blob",
            poem::get(serve_blob).data(blobs),
        )
        .nest("/", service.with(Tracing));
    let api_routes = Route::new().nest(
        "/",
        api_routes.with(crate::auth::RequireSession::new(auth_store)),
    );
    let routes = Route::new()
        .nest(MOUNT, api_routes)
        .at(
            "/mcp",
            crate::mcp::endpoint(mcp_store).with(crate::auth::RequireSession::new(mcp_auth_store)),
        )
        .at("/openapi.json", specification)
        .at("/*path", poem::get(crate::web::serve));
    match github_auth {
        Some(auth) => routes.nest("/auth", crate::auth::routes(auth)),
        None => routes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Run;
    use poem::test::TestClient;

    async fn routes_for_test() -> Route {
        let store = Arc::new(
            Store::open("sqlite::memory:", 0)
                .await
                .expect("store opens"),
        );
        routes(store, PathBuf::from(".tmp/analysis/test"), None)
    }

    async fn authenticated_routes() -> (Route, Arc<Store>, User, String) {
        let store = Arc::new(
            Store::open("sqlite::memory:", 0)
                .await
                .expect("store opens"),
        );
        let user = store
            .register_user("https://github.com", "owner", "Owner", true)
            .await
            .expect("registers")
            .expect("allows owner");
        let session = "test-session".to_owned();
        store
            .create_session(
                user.id,
                &blake3::hash(session.as_bytes()).to_hex().to_string(),
                Timestamp::now() + std::time::Duration::from_secs(60),
            )
            .await
            .expect("stores session");
        (
            routes(
                Arc::clone(&store),
                PathBuf::from(".tmp/analysis/test"),
                None,
            ),
            store,
            user,
            session,
        )
    }

    #[tokio::test]
    async fn health_reports_the_crate_version() {
        let response = TestClient::new(routes_for_test().await)
            .get("/api/health")
            .send()
            .await;

        response.assert_status_is_ok();
        let body = response.json().await;
        body.value().object().get("status").assert_string("ok");
        body.value().object().get("version").assert_string(VERSION);
    }

    #[tokio::test]
    async fn specification_names_the_api() {
        let response = TestClient::new(routes_for_test().await)
            .get("/openapi.json")
            .send()
            .await;

        response.assert_status_is_ok();
        response
            .json()
            .await
            .value()
            .object()
            .get("info")
            .object()
            .get("title")
            .assert_string(TITLE);
    }

    #[tokio::test]
    async fn api_tokens_are_issued_once_and_list_only_metadata() {
        let (routes, _, _, session) = authenticated_routes().await;
        let client = TestClient::new(routes);
        let response = client
            .post("/api/tokens")
            .header("cookie", format!("__Host-error-menu-session={session}"))
            .body_json(&serde_json::json!({"name": "deploy", "expires_at": null}))
            .send()
            .await;
        response.assert_status(StatusCode::CREATED);
        let created = response.json().await;
        let created = created.value().object();
        created.get("name").assert_string("deploy");
        let token = created.get("token").string();
        assert!(token.starts_with("em_pat_"));
        assert!(created.get_opt("created_at").is_some());
        assert!(created.get_opt("expires_at").is_none());

        let response = client
            .get("/api/tokens")
            .header("cookie", format!("__Host-error-menu-session={session}"))
            .send()
            .await;
        response.assert_status_is_ok();
        let listed = response.json().await;
        let tokens = listed.value().object().get("tokens").object_array();
        assert_eq!(tokens.len(), 1);
        tokens[0].get("name").assert_string("deploy");
        assert!(tokens[0].get_opt("token").is_none());
        assert!(tokens[0].get_opt("token_hash").is_none());
    }

    #[tokio::test]
    async fn api_tokens_can_only_be_revoked_by_their_owner() {
        let (routes, store, owner, session) = authenticated_routes().await;
        let other = store
            .register_user("https://github.com", "other", "Other", true)
            .await
            .expect("registers")
            .expect("allows other user");
        let token = "em_pat_other";
        let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        store
            .create_api_token(other.id, &token_hash, "other-token", None)
            .await
            .expect("stores other token");
        let client = TestClient::new(routes);
        client
            .delete("/api/tokens/other-token")
            .header("cookie", format!("__Host-error-menu-session={session}"))
            .send()
            .await
            .assert_status(StatusCode::NOT_FOUND);
        assert!(
            store
                .user_for_api_token(&token_hash, Timestamp::now())
                .await
                .expect("reads token")
                .is_some()
        );
        assert!(
            !store
                .revoke_api_token(owner.id, "other-token")
                .await
                .expect("keeps other token")
        );
    }
    fn run_record(status: RunStatus, severities: &[Severity]) -> crate::store::AnalysisRunRecord {
        let run_id = Id::from_raw(1);

        crate::store::AnalysisRunRecord {
            run: Run {
                id: run_id,
                snapshot_id: Id::from_raw(1),
                analyzer: "lockfile-delta".to_owned(),
                status,
                compared_against: None,
                started_at: jiff::Timestamp::UNIX_EPOCH,
                finished_at: None,
            },
            findings: severities
                .iter()
                .map(|severity| Finding {
                    movement: None,
                    id: Id::from_raw(1),
                    run_id,
                    issue_id: Id::from_raw(1),
                    fingerprint: crate::finding::fingerprint::Fingerprint {
                        version: 1,
                        hash: "hash".to_owned(),
                        canonical: "canonical".to_owned(),
                    },
                    location: Location::File {
                        path: crate::vcs::RepoPath::new("pnpm-lock.yaml").expect("path parses"),
                        span: None,
                    },
                    severity: *severity,
                    confidence: crate::confidence::Confidence::new(1.0)
                        .expect("confidence is in range"),
                    attribution: crate::finding::Attribution::Introduced,
                    title: "title".to_owned(),
                    detail: "detail".to_owned(),
                })
                .collect(),
            signals: Vec::new(),
        }
    }

    #[test]
    fn an_analyzer_that_could_not_finish_is_alarming() {
        let runs = [run_record(RunStatus::TimedOut, &[])];

        assert_eq!(analysis_status(&runs), AnalysisStatus::Alarming);
    }

    #[test]
    fn a_high_finding_outranks_a_medium_one() {
        let runs = [run_record(
            RunStatus::Succeeded,
            &[Severity::Medium, Severity::High],
        )];

        assert_eq!(analysis_status(&runs), AnalysisStatus::Alarming);
    }

    #[test]
    fn a_medium_finding_needs_attention() {
        let runs = [run_record(
            RunStatus::Succeeded,
            &[Severity::Info, Severity::Medium],
        )];

        assert_eq!(analysis_status(&runs), AnalysisStatus::Attention);
    }

    #[test]
    fn lockfile_context_alone_stays_clear() {
        let runs = [run_record(
            RunStatus::Succeeded,
            &[Severity::Info, Severity::Low, Severity::Info],
        )];

        assert_eq!(analysis_status(&runs), AnalysisStatus::Clear);
    }
}
