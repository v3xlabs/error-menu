use std::path::{Path, PathBuf};

use crate::database::Database;
use crate::forge::reader::ForgeReader;

/// Everything a request handler or a worker tick needs: the database, the forge client,
/// and the two directories error.menu writes into.
pub struct AppState {
    pub database: Database,
    pub forge: ForgeReader,
    pub mirrors: PathBuf,
    pub avatars: PathBuf,
}

impl AppState {
    pub fn new(database: Database, forge: ForgeReader, data_root: &Path) -> Self {
        Self {
            database,
            forge,
            mirrors: data_root.join("mirrors"),
            avatars: data_root.join("avatars"),
        }
    }
}
