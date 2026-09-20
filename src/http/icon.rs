use std::sync::Arc;

use poem::http::StatusCode;
use poem::web::{Data, Path, Query};
use poem::{Request, Response};

use crate::app::AppState;
use crate::http::api::{ProjectAccess, ProjectPermission, project_access};
use crate::http::auth::CurrentUser;
use crate::prelude::*;
use crate::project::icon::{MAX_ICON_BYTES, svg_is_safe};
use crate::vcs::mirror::Mirror;

#[derive(serde::Deserialize)]
pub struct BlobQuery {
    path: String,
}

#[poem::handler]
pub async fn serve_blob(
    Path(project_id): Path<String>,
    Query(query): Query<BlobQuery>,
    Data(state): Data<&Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    request: &Request,
) -> Response {
    let Ok(path) = RepoPath::new(&query.path) else {
        return status(StatusCode::NOT_FOUND);
    };
    if !is_image_path(path.as_str()) {
        return status(StatusCode::NOT_FOUND);
    }

    serve_image(state, &user, &project_id, Wanted::Blob(path), request).await
}

#[poem::handler]
pub async fn serve_icon(
    Path((project_id, scheme)): Path<(String, String)>,
    Data(state): Data<&Arc<AppState>>,
    CurrentUser(user): CurrentUser,
    request: &Request,
) -> Response {
    serve_image(state, &user, &project_id, Wanted::Icon(scheme), request).await
}

/// A picture is either asked for by path, while a reader is choosing one, or by scheme,
/// once the project has a mark. Both read one blob out of the default branch.
enum Wanted {
    Blob(RepoPath),
    Icon(String),
}

impl Wanted {
    fn path(self, project: &Project) -> Option<RepoPath> {
        match self {
            Self::Blob(path) => Some(path),
            Self::Icon(scheme) => match scheme.as_str() {
                "light" => project.icon.light.clone(),
                "dark" => project.icon.dark.clone(),
                _ => None,
            },
        }
    }
}

async fn serve_image(
    state: &AppState,
    user: &User,
    project_id: &str,
    wanted: Wanted,
    request: &Request,
) -> Response {
    let Ok(project_id) = project_id.parse::<Id<Project>>() else {
        return status(StatusCode::NOT_FOUND);
    };
    let project = match project_access(state, user, project_id, ProjectPermission::Viewer).await {
        ProjectAccess::Allowed { project, .. } => project,
        ProjectAccess::Forbidden => return status(StatusCode::FORBIDDEN),
        ProjectAccess::Missing => return status(StatusCode::NOT_FOUND),
        ProjectAccess::Failed(_) => return status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let Some(path) = wanted.path(&project) else {
        return status(StatusCode::NOT_FOUND);
    };
    let head = match Snapshot::default_branch_head(&state.database, project_id).await {
        Ok(Some(head)) => head,
        Ok(None) => return status(StatusCode::NOT_FOUND),
        Err(_) => return status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let Ok(mirror) = Mirror::open(&state.mirrors, &project.remote).await else {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let bytes = match mirror.bytes_at(&head, &path, MAX_ICON_BYTES).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return status(StatusCode::NOT_FOUND),
        Err(_) => return status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let Some(content_type) = image_content_type(path.as_str()) else {
        return status(StatusCode::NOT_FOUND);
    };
    if content_type == "image/svg+xml" && !svg_is_safe(&bytes) {
        return status(StatusCode::NOT_FOUND);
    }

    image_response(request, content_type, bytes)
}

/// Repository content is drawn by the browser, so it is served inert: no script, no
/// embedding, and never sniffed into another type.
fn image_response(request: &Request, content_type: &str, bytes: Vec<u8>) -> Response {
    let etag = format!("\"{}\"", &blake3::hash(&bytes).to_hex()[..16]);
    let known = request
        .headers()
        .get("if-none-match")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag);
    let response = Response::builder()
        .header("etag", etag)
        .header("cache-control", "no-cache")
        .header("content-security-policy", "default-src 'none'; sandbox")
        .header("x-content-type-options", "nosniff");

    if known {
        return response.status(StatusCode::NOT_MODIFIED).finish();
    }

    response.content_type(content_type).body(bytes)
}

fn status(status: StatusCode) -> Response {
    Response::builder().status(status).finish()
}

/// What a repository file may be served as. A file error.menu cannot name is not served
/// at all, so nothing here is ever sniffed into a type the browser would run.
const IMAGE_TYPES: [(&str, &str); 7] = [
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
];

fn image_content_type(path: &str) -> Option<&'static str> {
    let extension = path.rsplit('.').next()?;

    IMAGE_TYPES
        .iter()
        .find(|(suffix, _)| extension.eq_ignore_ascii_case(suffix))
        .map(|(_, content_type)| *content_type)
}

pub fn is_image_path(path: &str) -> bool {
    image_content_type(path).is_some()
}
