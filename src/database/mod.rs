//! The connection to SQLite and the errors reading it can produce. Queries do not live
//! here: each one is a method on the type it reads or writes.

pub mod codec;

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqlitePool, Transaction};

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
            // A write-ahead log already survives a process that dies, and every row here
            // is either re-derivable or a credential a reader can make again, so waiting
            // for the platter on each commit buys nothing and costs seconds on a network
            // volume.
            .synchronous(SqliteSynchronous::Normal)
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

    /// A transaction that will write before it commits.
    ///
    /// Every one of ours reads and then writes. Started deferred, the write lock is taken
    /// late, and SQLite answers a late upgrade with `SQLITE_BUSY` immediately instead of
    /// waiting out `busy_timeout`, so a reader that arrives while the queue is writing is
    /// refused rather than delayed. Claiming the writer up front makes the wait happen.
    pub async fn write(&self) -> Result<Transaction<'static, Sqlite>, DatabaseError> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
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

    /// Whether the database refused because another writer held it. SQLite reports that
    /// as `SQLITE_BUSY` or `SQLITE_LOCKED`, each also in extended forms that carry the
    /// primary code in its low byte. It passes, so a caller may say "come back" rather
    /// than "this is broken".
    pub fn is_contended(&self) -> bool {
        self.database_code()
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| matches!(code & 0xff, 5 | 6))
    }
}
