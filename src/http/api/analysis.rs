use std::collections::BTreeMap;
use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::analysis::{CompletedRun, SnapshotAnalysis, ci_checks, runner};
use crate::app::AppState;
use crate::database::codec::StoredAs;
use crate::forge::ChangeState;
use crate::http::MOUNT;
use crate::http::api::{
    Error, ProjectAccess, ProjectPermission, forbidden, missing_project, project_access,
};
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::registry::PackageFacts;
use crate::worker::discovery;

/// Registry facts for one project, keyed by the coordinate a finding names.
type FactsIndex = BTreeMap<(&'static str, String, String), PackageFacts>;

pub struct AnalysisApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl AnalysisApi {
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
        match project_access(&self.state, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => return ListAnalysesResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => {
                return ListAnalysesResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ListAnalysesResponse::Failed(Json(Error { message }));
            }
        }
        let facts = match facts_index(&self.state.database, project_id).await {
            Ok(facts) => facts,
            Err(message) => return ListAnalysesResponse::Failed(Json(Error { message })),
        };
        match SnapshotAnalysis::for_project(&self.state.database, project_id).await {
            Ok(analyses) => ListAnalysesResponse::Found(Json(AnalysesOutput {
                analyses: analyses
                    .into_iter()
                    .map(|analysis| analysis_output(analysis, &facts))
                    .collect(),
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
        match project_access(&self.state, &user, project_id, ProjectPermission::Operator).await {
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
        let discovery = match discovery::run(&self.state, project_id).await {
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
        match discovery_output(&self.state.database, project_id, discovery).await {
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
        match project_access(&self.state, &user, project_id, ProjectPermission::Operator).await {
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
        match runner::run(&self.state, project_id, base, head).await {
            Ok(analysis) => {
                let facts = match facts_index(&self.state.database, project_id).await {
                    Ok(facts) => facts,
                    Err(message) => {
                        return AnalyzeProjectResponse::Failed(Json(Error { message }));
                    }
                };
                match SnapshotAnalysis::for_project(&self.state.database, project_id).await {
                    Ok(analyses) => match analyses
                        .into_iter()
                        .find(|stored| stored.snapshot.id == analysis.snapshot.id)
                    {
                        Some(stored) => {
                            AnalyzeProjectResponse::Created(Json(analysis_output(stored, &facts)))
                        }
                        None => AnalyzeProjectResponse::Failed(Json(Error {
                            message: "analysis was not stored".to_owned(),
                        })),
                    },
                    Err(error) => AnalyzeProjectResponse::Failed(Json(Error {
                        message: error.to_string(),
                    })),
                }
            }
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
        match project_access(&self.state, &user, project_id, ProjectPermission::Operator).await {
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
        let snapshot = match discovery::scan_change(&self.state, project_id, number.0).await {
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

        let facts = match facts_index(&self.state.database, project_id).await {
            Ok(facts) => facts,
            Err(message) => return AnalyzeProjectResponse::Failed(Json(Error { message })),
        };

        match SnapshotAnalysis::for_project(&self.state.database, project_id).await {
            Ok(mut analyses) => match take_analysis(&mut analyses, snapshot.id, &facts) {
                Ok(output) => AnalyzeProjectResponse::Created(Json(output)),
                Err(message) => AnalyzeProjectResponse::Failed(Json(Error { message })),
            },
            Err(error) => AnalyzeProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
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
    links: PackageLinksOutput,
    facts: Option<PackageFactsOutput>,
}

/// Built on read and never stored. A URL is derived from the coordinate and the origin, so
/// a stored copy would be a second answer that ages.
#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct PackageLinksOutput {
    registry: Option<String>,
    docs: Option<String>,
    source: Option<String>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct PackageFactsOutput {
    size_bytes: Option<u64>,
    install_bytes: Option<u64>,
    dependency_count: Option<u32>,
    downloads_week: Option<u64>,
    vulnerabilities: Option<u32>,
    vulnerabilities_high: Option<u32>,
    license: Option<String>,
    withdrawn: Option<String>,
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
    Draft,
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

async fn discovery_output(
    database: &Database,
    project_id: Id<Project>,
    discovery: discovery::Discovery,
) -> Result<DiscoveryOutput, String> {
    let mut analyses = SnapshotAnalysis::for_project(database, project_id)
        .await
        .map_err(|error| error.to_string())?;
    let facts = facts_index(database, project_id).await?;
    let default_branch = take_analysis(
        &mut analyses,
        discovery.default_branch.analysis.snapshot.id,
        &facts,
    )?;
    let mut pull_requests = Vec::with_capacity(discovery.changes.len());
    for change in discovery.changes {
        pull_requests.push(take_analysis(&mut analyses, change.snapshot.id, &facts)?);
    }

    Ok(DiscoveryOutput {
        default_branch,
        pull_requests,
    })
}

/// Every fact this project's findings can be joined to, read once so a response that lists
/// hundreds of findings makes one query and not hundreds.
async fn facts_index(database: &Database, project_id: Id<Project>) -> Result<FactsIndex, String> {
    PackageFacts::for_project(database, project_id)
        .await
        .map_err(|error| error.to_string())
}

fn take_analysis(
    analyses: &mut Vec<SnapshotAnalysis>,
    snapshot_id: Id<Snapshot>,
    facts: &FactsIndex,
) -> Result<AnalysisOutput, String> {
    let Some(index) = analyses
        .iter()
        .position(|analysis| analysis.snapshot.id == snapshot_id)
    else {
        return Err("discovery analysis was not stored".to_owned());
    };

    Ok(analysis_output(analyses.remove(index), facts))
}

fn analysis_output(analysis: SnapshotAnalysis, facts: &FactsIndex) -> AnalysisOutput {
    let status = analysis_status(&analysis.runs);
    let analyzers = analysis
        .runs
        .into_iter()
        .map(|stored_run| {
            let findings = stored_run
                .findings
                .into_iter()
                .map(|finding| finding_output(finding, facts))
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
        ChangeState::Draft => ChangeStateOutput::Draft,
        ChangeState::Open => ChangeStateOutput::Open,
        ChangeState::Closed => ChangeStateOutput::Closed,
        ChangeState::Merged => ChangeStateOutput::Merged,
    }
}

fn analysis_status(runs: &[CompletedRun]) -> AnalysisStatus {
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

fn finding_output(finding: Finding, facts: &FactsIndex) -> FindingOutput {
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
            origin,
            integrity: _,
        } => {
            let known = facts.get(&(ecosystem.stored(), name.clone(), version.clone()));

            (
                path.to_string(),
                None,
                None,
                Some(PackageOutput {
                    ecosystem: ecosystem_output(ecosystem),
                    links: links_of(ecosystem, &name, &version, origin.as_ref(), known),
                    facts: known.map(facts_output),
                    name,
                    version,
                }),
            )
        }
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

fn links_of(
    ecosystem: Ecosystem,
    name: &str,
    version: &str,
    origin: Option<&PackageOrigin>,
    facts: Option<&PackageFacts>,
) -> PackageLinksOutput {
    // A registry page is only the right page when the package came from that registry. A
    // git pin carries the same name as a published crate and describes something else.
    let public = matches!(origin, Some(PackageOrigin::PublicRegistry));

    PackageLinksOutput {
        registry: match (ecosystem, public) {
            (Ecosystem::Cargo, true) => Some(format!("https://crates.io/crates/{name}/{version}")),
            (Ecosystem::Npm, true) => Some(format!("https://npmx.dev/package/{name}/v/{version}")),
            _ => None,
        },
        docs: facts.and_then(|facts| facts.documentation.clone()),
        source: source_link(ecosystem, version, origin, facts),
    }
}

/// A flake input pins a revision, and the revision is its version, so the link goes to the
/// tree the lockfile actually names.
fn source_link(
    ecosystem: Ecosystem,
    version: &str,
    origin: Option<&PackageOrigin>,
    facts: Option<&PackageFacts>,
) -> Option<String> {
    match origin {
        Some(PackageOrigin::Remote { url }) if ecosystem == Ecosystem::Nix => {
            Some(format!("{url}/tree/{version}"))
        }
        Some(PackageOrigin::Remote { url }) => Some(url.clone()),
        _ => facts.and_then(|facts| facts.repository.clone()),
    }
}

fn facts_output(facts: &PackageFacts) -> PackageFactsOutput {
    PackageFactsOutput {
        size_bytes: facts.size_bytes,
        install_bytes: facts.install_bytes,
        dependency_count: facts.dependency_count,
        downloads_week: facts.downloads_week,
        vulnerabilities: facts.vulnerabilities,
        vulnerabilities_high: facts.vulnerabilities_high,
        license: facts.license.clone(),
        withdrawn: facts.withdrawn.clone(),
    }
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
        SignalValue::Score(score) => (
            SignalValueKindOutput::Score,
            Some(f64::from(score.value())),
            None,
            None,
        ),
        SignalValue::Flag(flag) => (SignalValueKindOutput::Flag, None, Some(flag), None),
        SignalValue::Count(count) => (SignalValueKindOutput::Count, None, None, Some(count)),
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
