use std::sync::Arc;

use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, Enum, Object, OpenApi};

use crate::app::AppState;
use crate::forge::github::installation::{GithubInstallation, full_name};
use crate::http::api::{
    Error, ProjectAccess, ProjectPermission, forbidden, missing_project, project_access,
};
use crate::http::auth::CurrentUser;
use crate::prelude::*;

pub struct ReportingApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl ReportingApi {
    /// Whether a project writes its verdict to its forge, and what the forge still needs
    /// before it can.
    #[oai(path = "/projects/:project_id/reporting", method = "get")]
    async fn reporting(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
    ) -> ReportingResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ReportingResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        match project_access(&self.state, &user, project_id, ProjectPermission::Viewer).await {
            ProjectAccess::Allowed { project, .. } => self.output(&project).await,
            ProjectAccess::Forbidden => ReportingResponse::Forbidden(Json(forbidden())),
            ProjectAccess::Missing => ReportingResponse::Missing(Json(missing_project())),
            ProjectAccess::Failed(message) => ReportingResponse::Failed(Json(Error { message })),
        }
    }

    /// Turns reporting on or off. Only an owner may, like every other project setting. The
    /// switch can be on before the app is installed; nothing is written until it is.
    #[oai(path = "/projects/:project_id/reporting", method = "put")]
    async fn set_reporting(
        &self,
        CurrentUser(user): CurrentUser,
        project_id: Path<String>,
        input: Json<SetForgeReporting>,
    ) -> ReportingResponse {
        let project_id = match project_id.0.parse::<Id<Project>>() {
            Ok(project_id) => project_id,
            Err(error) => {
                return ReportingResponse::Invalid(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        let mut project =
            match project_access(&self.state, &user, project_id, ProjectPermission::Owner).await {
                ProjectAccess::Allowed { project, .. } => project,
                ProjectAccess::Forbidden => {
                    return ReportingResponse::Forbidden(Json(forbidden()));
                }
                ProjectAccess::Missing => {
                    return ReportingResponse::Missing(Json(missing_project()));
                }
                ProjectAccess::Failed(message) => {
                    return ReportingResponse::Failed(Json(Error { message }));
                }
            };
        if input.0.enabled && !self.reachable(&project) {
            return ReportingResponse::Invalid(Json(Error {
                message: "this project cannot report: the server has no GitHub App, or the \
                          repository is not on github.com"
                    .to_owned(),
            }));
        }
        if let Err(error) = project
            .report_to_forge(&self.state.database, input.0.enabled)
            .await
        {
            return ReportingResponse::Failed(Json(Error {
                message: error.to_string(),
            }));
        }

        self.output(&project).await
    }
}

impl ReportingApi {
    fn reachable(&self, project: &Project) -> bool {
        self.state.github_app.is_some() && full_name(&project.remote, project.forge_kind).is_some()
    }

    async fn output(&self, project: &Project) -> ReportingResponse {
        let (Some(app), true) = (self.state.github_app.as_deref(), self.reachable(project)) else {
            return ReportingResponse::Found(Json(ForgeReportingOutput {
                enabled: project.reports_to_forge,
                state: ForgeReportingState::Unavailable,
                install_url: None,
            }));
        };
        let installed = match GithubInstallation::for_project(&self.state.database, project).await {
            Ok(installation) => installation.is_some(),
            Err(error) => {
                return ReportingResponse::Failed(Json(Error {
                    message: error.to_string(),
                }));
            }
        };
        // The page still works without the link: the state says what is missing, and the
        // app can be found on GitHub by hand.
        let install_url = match app.install_url().await {
            Ok(url) => Some(url),
            Err(error) => {
                tracing::warn!(%error, "could not read the GitHub App's install page");
                None
            }
        };

        ReportingResponse::Found(Json(ForgeReportingOutput {
            enabled: project.reports_to_forge,
            state: if installed {
                ForgeReportingState::Installed
            } else {
                ForgeReportingState::NotInstalled
            },
            install_url,
        }))
    }
}

#[derive(Debug, Enum)]
#[oai(rename_all = "snake_case")]
pub enum ForgeReportingState {
    /// The server has no GitHub App, or the repository is not on github.com.
    Unavailable,
    /// The GitHub App is not installed on the repository yet.
    NotInstalled,
    Installed,
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
pub struct ForgeReportingOutput {
    /// Whether the project writes its verdicts to the forge. Takes effect once installed.
    enabled: bool,
    state: ForgeReportingState,
    /// Where an account installs the GitHub App on the repository.
    install_url: Option<String>,
}

#[derive(Debug, Object)]
struct SetForgeReporting {
    enabled: bool,
}

#[allow(dead_code)]
#[derive(ApiResponse)]
enum ReportingResponse {
    #[oai(status = 200)]
    Found(Json<ForgeReportingOutput>),
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
