use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Notify;

use crate::database::Database;
use crate::forge::github::app::GithubApp;
use crate::forge::reader::ForgeReader;
use crate::project::activity::ProjectActivity;

/// Everything a request handler or a worker tick needs: the database, the forge client,
/// the GitHub App when the server has one, the in-memory project activity, and the two
/// directories error.menu writes into.
///
/// The app writes checks and the worker never does: the GitHub App lives here, on the
/// side that holds the database, and a worker that splits off takes none of it.
pub struct AppState {
    pub database: Database,
    pub forge: ForgeReader,
    pub github_app: Option<Arc<GithubApp>>,
    pub activity: ProjectActivity,
    /// Wakes the queue before its next tick, so a webhook's job starts when it arrives.
    pub queue_wake: Notify,
    pub mirrors: PathBuf,
    pub avatars: PathBuf,
}

impl AppState {
    pub fn new(
        database: Database,
        forge: ForgeReader,
        github_app: Option<Arc<GithubApp>>,
        activity: ProjectActivity,
        data_root: &Path,
    ) -> Self {
        Self {
            database,
            forge,
            github_app,
            activity,
            queue_wake: Notify::new(),
            mirrors: data_root.join("mirrors"),
            avatars: data_root.join("avatars"),
        }
    }
}
