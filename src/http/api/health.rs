use std::sync::Arc;

use poem_openapi::payload::Json;
use poem_openapi::{Object, OpenApi};

use crate::app::AppState;
use crate::http::VERSION;

pub struct HealthApi {
    pub state: Arc<AppState>,
}

#[OpenApi]
impl HealthApi {
    #[oai(path = "/health", method = "get")]
    async fn health(&self) -> Json<Health> {
        Json(Health {
            status: "ok".to_owned(),
            version: VERSION.to_owned(),
        })
    }
}

#[derive(Debug, Object)]
#[oai(skip_serializing_if_is_none)]
struct Health {
    status: String,
    version: String,
}
