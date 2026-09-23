use std::path::{Path, PathBuf};

use crate::database::Database;
use crate::forge::reader::ForgeReader;
use crate::project::activity::ProjectActivity;

/// Everything a request handler or a worker tick needs: the database, the forge client,
/// the in-memory project activity, and the two directories error.menu writes into.
pub struct AppState {
    pub database: Database,
    pub forge: ForgeReader,
    pub activity: ProjectActivity,
    pub mirrors: PathBuf,
    pub avatars: PathBuf,
}

impl AppState {
    pub fn new(
        database: Database,
        forge: ForgeReader,
        activity: ProjectActivity,
        data_root: &Path,
    ) -> Self {
        Self {
            database,
            forge,
            activity,
            mirrors: data_root.join("mirrors"),
            avatars: data_root.join("avatars"),
        }
    }
}
