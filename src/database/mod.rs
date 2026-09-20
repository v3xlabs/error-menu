//! The connection to SQLite and the errors reading it can produce. Queries do not live
//! here: each one is a method on the type it reads or writes.

pub mod codec;

use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::id::IdGenerator;

pub struct Database {
    pub pool: SqlitePool,
    pub ids: IdGenerator,
}

impl Database {
    pub async fn open(url: &str, node: u16) -> Result<Self, DatabaseError> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let max_connections = if url == "sqlite::memory:" { 1 } else { 4 };
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            ids: IdGenerator::new(node),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("database: {0}")]
    Query(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("stored {field} is not readable: {value:?}")]
    Unreadable { field: &'static str, value: String },
    #[error("analyzer is not implemented: {0}")]
    UnknownAnalyzer(String),
}

impl DatabaseError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Query(_) => "database",
            Self::Migration(_) => "migration",
            Self::Unreadable { .. } => "unreadable",
            Self::UnknownAnalyzer(_) => "unknown_analyzer",
        }
    }

    pub fn database_code(&self) -> Option<String> {
        let Self::Query(sqlx::Error::Database(error)) = self else {
            return None;
        };

        error.code().map(|code| code.into_owned())
    }
}
