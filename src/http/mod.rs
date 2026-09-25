pub mod api;
pub mod assets;
pub mod auth;
pub mod avatar;
pub mod icon;
pub mod mcp;
pub mod oauth;
pub mod trace;
pub mod webhook;

use std::sync::Arc;

use poem::{Endpoint, EndpointExt, Route};
use poem_openapi::OpenApiService;

use crate::app::AppState;
use crate::http::api::{
    analysis, health, job, member, organization, project, reporting, repository, token, user,
};

const TITLE: &str = "error.menu";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MOUNT: &str = "/api";

pub fn routes(
    state: Arc<AppState>,
    github_auth: Option<Arc<oauth::GithubAuth>>,
) -> impl Endpoint<Output = poem::Response> {
    let service = OpenApiService::new(
        (
            health::HealthApi {
                state: Arc::clone(&state),
            },
            project::ProjectApi {
                state: Arc::clone(&state),
            },
            job::JobApi {
                state: Arc::clone(&state),
            },
            analysis::AnalysisApi {
                state: Arc::clone(&state),
            },
            member::MemberApi {
                state: Arc::clone(&state),
            },
            organization::OrganizationApi {
                state: Arc::clone(&state),
            },
            repository::RepositoryApi {
                state: Arc::clone(&state),
            },
            reporting::ReportingApi {
                state: Arc::clone(&state),
            },
            token::TokenApi {
                state: Arc::clone(&state),
            },
            user::UserApi {
                state: Arc::clone(&state),
            },
        ),
        TITLE,
        VERSION,
    )
    .server(MOUNT);
    let specification = service.spec_endpoint();

    let api_routes = Route::new()
        .at(
            "/avatars/:identity",
            poem::get(avatar::serve).data(Arc::clone(&state)),
        )
        .at(
            "/projects/:project_id/icon/:scheme",
            poem::get(icon::serve_icon).data(Arc::clone(&state)),
        )
        .at(
            "/projects/:project_id/blob",
            poem::get(icon::serve_blob).data(Arc::clone(&state)),
        )
        .nest("/", service);
    let api_routes = Route::new().nest(
        "/",
        api_routes.with(auth::RequireSession::new(Arc::clone(&state))),
    );
    let routes = Route::new()
        .nest(MOUNT, api_routes)
        // Outside the session guard: GitHub signs a delivery instead of signing in.
        .at(
            "/forge/github/events",
            poem::post(webhook::github).data(Arc::clone(&state)),
        )
        .at(
            "/mcp",
            mcp::endpoint(Arc::clone(&state)).with(auth::RequireSession::new(state)),
        )
        .at("/openapi.json", specification)
        .at("/*path", poem::get(assets::serve));
    let routes = match github_auth {
        Some(auth) => routes.nest("/auth", oauth::routes(auth)),
        None => routes,
    };

    routes.with(trace::RequestTrace)
}
