use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::analysis::{
    ci_checks, hygiene, links, lockfile, manifest, repository_controls, secret, workflow,
};
use crate::app::AppState;
use crate::forge::ForgeKind;
use crate::http::MOUNT;
use crate::http::api::member::ProjectRoleOutput;
use crate::http::api::{
    CustodyAccess, Error, OrganizationAccess, OrganizationPermission, ProjectAccess,
    ProjectPermission, custody_access, forbidden, missing_organization, missing_project,
    organization_access, project_access,
};
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::project::{NewProject, ProjectAnalyzerTone, ProjectBranchSummary};

pub struct ProjectApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl ProjectApi {
    #[oai(path = "/projects", method = "get")]
    async fn list_projects(&self, CurrentUser(user): CurrentUser) -> ListProjectsResponse {
        let summaries = match Project::summaries_for(&self.state.database, &user).await {
            Ok(summaries) => summaries,
            Err(error) => {
                return ListProjectsResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let projects = summaries
            .into_iter()
            .map(|summary| {
                build_project_output(
                    summary.project,
                    summary.organization_name,
                    summary.viewer_role,
                    summary.analyzers,
                    summary.default_branch.map(branch_output),
                )
            })
            .collect::<Result<Vec<_>, _>>();

        match projects {
            Ok(projects) => ListProjectsResponse::Found(Json(ProjectsOutput { projects })),
            Err(error) => ListProjectsResponse::Failed(Json(Error {
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
        let input = input.0;
        let organization_id = match input.organization_id.parse::<Id<Organization>>() {
            Ok(organization_id) => organization_id,
            Err(error) => {
                return CreateProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match organization_access(
            &self.state,
            &user,
            organization_id,
            OrganizationPermission::Owner,
        )
        .await
        {
            OrganizationAccess::Allowed { .. } => {}
            OrganizationAccess::Forbidden => {
                return CreateProjectResponse::Forbidden(Json(forbidden()));
            }
            OrganizationAccess::Missing => {
                return CreateProjectResponse::Invalid(Json(missing_organization()));
            }
            OrganizationAccess::Failed(message) => {
                return CreateProjectResponse::Failed(Json(Error { message }));
            }
        }
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
        let project = match Project::create(
            &self.state.database,
            NewProject {
                organization_id,
                owner_id: user.id,
                name: input.name.trim(),
                remote,
                forge_kind: input.forge.unwrap_or(Forge::Auto).into_kind(),
                uses_default_analyzers: input.uses_default_analyzers,
                analyzers: &analyzers,
            },
        )
        .await
        {
            Ok(project) => project,
            Err(error) => return failed(error),
        };

        match project_output(&self.state.database, project, ProjectRole::Owner).await {
            Ok(project) => CreateProjectResponse::Created(Json(project)),
            Err(error) => failed(error),
        }
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
        match project_access(&self.state, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { project, role } => {
                match project_output(&self.state.database, project, role).await {
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
        let project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => {
                    return GetProjectResponse::Forbidden(Json(forbidden()));
                }
                ProjectAccess::Missing => {
                    return GetProjectResponse::Missing(Json(missing_project()));
                }
                ProjectAccess::Failed(message) => {
                    return GetProjectResponse::Failed(Json(Error { message }));
                }
            };
        let input = input.0;
        let analyzers = input
            .analyzers
            .iter()
            .copied()
            .map(Analyzer::as_str)
            .collect::<Vec<_>>();
        if let Err(error) = project
            .set_analyzers(
                &self.state.database,
                input.uses_default_analyzers,
                &analyzers,
            )
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }

        project_response(&self.state.database, project_id, ProjectRole::Owner).await
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
        let project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => {
                    return GetProjectResponse::Forbidden(Json(forbidden()));
                }
                ProjectAccess::Missing => {
                    return GetProjectResponse::Missing(Json(missing_project()));
                }
                ProjectAccess::Failed(message) => {
                    return GetProjectResponse::Failed(Json(Error { message }));
                }
            };
        let description = input.0.description.filter(|text| !text.trim().is_empty());
        if let Err(error) = project
            .describe(&self.state.database, description.as_deref(), &project.icon)
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }

        project_response(&self.state.database, project_id, ProjectRole::Owner).await
    }

    #[oai(path = "/projects/:project_id/organization", method = "put")]
    async fn move_project(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<MoveProject>,
    ) -> GetProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let organization_id = match input.0.organization_id.parse::<Id<Organization>>() {
            Ok(organization_id) => organization_id,
            Err(error) => {
                return GetProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let (project, from) = match custody_access(&self.state, &user, project_id).await {
            CustodyAccess::Allowed {
                project,
                organization,
            } => (project, organization),
            CustodyAccess::Forbidden => {
                return GetProjectResponse::Forbidden(Json(Error {
                    message: "moving a project needs an owner grant on the organization \
                              that holds it"
                        .to_owned(),
                }));
            }
            CustodyAccess::Missing => {
                return GetProjectResponse::Missing(Json(missing_project()));
            }
            CustodyAccess::Failed(message) => {
                return GetProjectResponse::Failed(Json(Error { message }));
            }
        };
        if project.organization_id == organization_id {
            return GetProjectResponse::Invalid(Json(Error {
                message: "the project is already in that organization".to_owned(),
            }));
        }
        let to = match organization_access(
            &self.state,
            &user,
            organization_id,
            OrganizationPermission::Owner,
        )
        .await
        {
            OrganizationAccess::Allowed { organization, .. } => organization,
            OrganizationAccess::Forbidden => {
                return GetProjectResponse::Forbidden(Json(Error {
                    message: "the destination organization needs an owner grant".to_owned(),
                }));
            }
            OrganizationAccess::Missing => {
                return GetProjectResponse::Missing(Json(missing_organization()));
            }
            OrganizationAccess::Failed(message) => {
                return GetProjectResponse::Failed(Json(Error { message }));
            }
        };
        if let Err(error) = project
            .transfer(&self.state.database, &from, &to, user.id)
            .await
        {
            return GetProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }

        project_response(&self.state.database, project_id, ProjectRole::Owner).await
    }

    /// Owner only, because the history names organizations the reader may hold no grant
    /// on, and it sits inside the owner's settings dialog.
    #[oai(path = "/projects/:project_id/transfers", method = "get")]
    async fn project_transfers(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> ProjectTransfersResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ProjectTransfersResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
            ProjectAccess::Allowed { .. } => {}
            ProjectAccess::Forbidden => {
                return ProjectTransfersResponse::Forbidden(Json(forbidden()));
            }
            ProjectAccess::Missing => {
                return ProjectTransfersResponse::Missing(Json(missing_project()));
            }
            ProjectAccess::Failed(message) => {
                return ProjectTransfersResponse::Failed(Json(Error { message }));
            }
        }
        match ProjectTransfer::for_project(&self.state.database, project_id).await {
            Ok(transfers) => ProjectTransfersResponse::Found(Json(ProjectTransfersOutput {
                transfers: transfers.into_iter().map(transfer_output).collect(),
            })),
            Err(error) => ProjectTransfersResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }

    #[oai(path = "/projects/:project_id", method = "delete")]
    async fn delete_project(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> DeleteProjectResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return DeleteProjectResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let project = match custody_access(&self.state, &user, project_id).await {
            CustodyAccess::Allowed { project, .. } => project,
            CustodyAccess::Forbidden => {
                return DeleteProjectResponse::Forbidden(Json(Error {
                    message: "deleting a project needs an owner grant on the organization \
                              that holds it"
                        .to_owned(),
                }));
            }
            CustodyAccess::Missing => {
                return DeleteProjectResponse::Missing(Json(missing_project()));
            }
            CustodyAccess::Failed(message) => {
                return DeleteProjectResponse::Failed(Json(Error { message }));
            }
        };
        match project.delete(&self.state.database).await {
            Ok(()) => DeleteProjectResponse::Deleted,
            Err(error) => DeleteProjectResponse::Failed(Json(Error {
                message: error.to_string(),
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[oai(rename_all = "snake_case")]
pub enum Analyzer {
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
pub enum Forge {
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
    organization_id: String,
    name: String,
    remote_url: String,
    uses_default_analyzers: bool,
    analyzers: Vec<Analyzer>,
    forge: Option<Forge>,
}

#[derive(Debug, Object)]
struct MoveProject {
    organization_id: String,
}

#[derive(Debug, Object)]
struct ProjectTransferOutput {
    transfer_id: String,
    from_organization_name: String,
    to_organization_name: String,
    moved_by: String,
    moved_by_name: String,
    moved_at: String,
}

#[derive(Debug, Object)]
struct ProjectTransfersOutput {
    transfers: Vec<ProjectTransferOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct SetProjectAnalyzers {
    uses_default_analyzers: bool,
    analyzers: Vec<Analyzer>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct ProjectOutput {
    project_id: String,
    organization_id: String,
    organization_name: String,
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
    /// How the default branch last scanned. Only the project list reports it, because it
    /// is the one view that shows many projects without opening any of them.
    default_branch: Option<ProjectBranchOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct ProjectBranchOutput {
    snapshot_id: String,
    analyzers: Vec<ProjectAnalyzerOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct ProjectAnalyzerOutput {
    analyzer: String,
    finding_count: u64,
    tone: AnalyzerToneOutput,
}

#[derive(Debug, Enum)]
#[oai(rename_all = "snake_case")]
pub enum AnalyzerToneOutput {
    Alarming,
    Attention,
    Clear,
    Running,
    Unscanned,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct ProjectsOutput {
    projects: Vec<ProjectOutput>,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct DescribeProject {
    description: Option<String>,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ListProjectsResponse {
    #[oai(status = 200)]
    Found(Json<ProjectsOutput>),
    #[oai(status = 401)]
    Unauthenticated(Json<Error>),
    #[oai(status = 500)]
    Failed(Json<Error>),
}

#[allow(dead_code)]
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
pub enum GetProjectResponse {
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

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ProjectTransfersResponse {
    #[oai(status = 200)]
    Found(Json<ProjectTransfersOutput>),
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
enum DeleteProjectResponse {
    #[oai(status = 204)]
    Deleted,
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

fn transfer_output(transfer: ProjectTransfer) -> ProjectTransferOutput {
    ProjectTransferOutput {
        transfer_id: transfer.id.encode(),
        from_organization_name: transfer.from_organization_name,
        to_organization_name: transfer.to_organization_name,
        moved_by: transfer.moved_by.encode(),
        moved_by_name: transfer.moved_by_name,
        moved_at: transfer.moved_at.to_string(),
    }
}

pub async fn project_response(
    database: &Database,
    project_id: Id<Project>,
    role: ProjectRole,
) -> GetProjectResponse {
    match Project::load(database, project_id).await {
        Ok(Some(project)) => match project_output(database, project, role).await {
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

fn failed(error: DatabaseError) -> CreateProjectResponse {
    CreateProjectResponse::Failed(Json(Error {
        message: error.to_string(),
    }))
}

async fn project_output(
    database: &Database,
    project: Project,
    viewer_role: ProjectRole,
) -> Result<ProjectOutput, DatabaseError> {
    let organization = Organization::load(database, project.organization_id)
        .await?
        .ok_or_else(|| DatabaseError::Unreadable {
            field: "projects.organization_id",
            value: project.organization_id.encode(),
        })?;
    let analyzers = project.effective_analyzers(database).await?;

    build_project_output(project, organization.name, viewer_role, analyzers, None)
}

fn build_project_output(
    project: Project,
    organization_name: String,
    viewer_role: ProjectRole,
    analyzers: Vec<String>,
    default_branch: Option<ProjectBranchOutput>,
) -> Result<ProjectOutput, DatabaseError> {
    let analyzers = analyzers
        .into_iter()
        .map(|analyzer| {
            Analyzer::from_str(&analyzer).ok_or(DatabaseError::UnknownAnalyzer(analyzer))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProjectOutput {
        project_id: project.id.encode(),
        organization_id: project.organization_id.encode(),
        organization_name,
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
        default_branch,
    })
}

fn branch_output(branch: ProjectBranchSummary) -> ProjectBranchOutput {
    ProjectBranchOutput {
        snapshot_id: branch.snapshot_id.encode(),
        analyzers: branch
            .analyzers
            .into_iter()
            .map(|analyzer| ProjectAnalyzerOutput {
                analyzer: analyzer.analyzer,
                finding_count: analyzer.finding_count,
                tone: match analyzer.tone {
                    ProjectAnalyzerTone::Alarming => AnalyzerToneOutput::Alarming,
                    ProjectAnalyzerTone::Attention => AnalyzerToneOutput::Attention,
                    ProjectAnalyzerTone::Clear => AnalyzerToneOutput::Clear,
                    ProjectAnalyzerTone::Running => AnalyzerToneOutput::Running,
                    ProjectAnalyzerTone::Unscanned => AnalyzerToneOutput::Unscanned,
                },
            })
            .collect(),
    }
}

/// The URL the mark is served from. The path in the repository is folded into it, because
/// the picture at `/icon/light` changes whenever the reader picks a different file, and a
/// URL that never changes would be answered from the browser cache with the old one.
fn icon_url(project_id: Id<Project>, scheme: &str, path: &RepoPath) -> String {
    let version = &blake3::hash(path.as_str().as_bytes()).to_hex()[..8];

    format!(
        "{MOUNT}/projects/{}/icon/{scheme}?v={version}",
        project_id.encode()
    )
}
