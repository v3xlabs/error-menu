use poem::http::StatusCode;
use poem::web::Path;
use poem::Response;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;
#[poem::handler]
pub async fn serve(Path(path): Path<String>) -> Response {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let (content_path, asset) = match Assets::get(path) {
        Some(asset) => (path, asset),
        None if !path.rsplit('/').next().is_some_and(|segment| segment.contains('.')) => {
            match Assets::get("index.html") {
                Some(asset) => ("index.html", asset),
                None => return Response::builder().status(StatusCode::NOT_FOUND).finish(),
            }
        }
        None => return Response::builder().status(StatusCode::NOT_FOUND).finish(),
    };

    Response::builder()
        .content_type(content_type(content_path))
        .body(asset.data.into_owned())
}
fn content_type(path: &str) -> &'static str {
    if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}
