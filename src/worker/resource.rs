//! What jobs share, and how many of them may use each thing at once.
//!
//! A job declares its resources when it is queued, and a worker only claims a job whose
//! resources all have room and none is blocked. That is the whole scheduler: work that waits
//! on crates.io never holds up discovery, and a registry that answers 429 stops only the jobs
//! that talk to it.

use jiff::Timestamp;

use crate::prelude::*;

const DISCOVERY: usize = 2;
const NPM: usize = 4;
/// crates.io asks API clients for one request a second (https://crates.io/data-access), and a
/// single holder that paces itself keeps to that without a shared clock.
const CRATES_IO: usize = 1;
const OSV: usize = 4;
/// Audits write runs, and SQLite has one writer, so running two gains nothing.
const AUDIT: usize = 1;

/// One worker for every slot a global resource offers, so every kind of work can always find
/// a worker however busy the others are. OSV is left out: only a crates.io job holds it, and
/// crates.io has fewer slots.
pub const POOL: usize = DISCOVERY + NPM + CRATES_IO + AUDIT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// One project's git mirror: two fetches of one mirror would race on its files.
    Mirror(Id<Project>),
    /// Cloning and analysing is disk and CPU, so only so many run at once.
    Discovery,
    /// registry.npmjs.org, api.npmjs.org and npmx.dev.
    Npm,
    /// The crates.io API and docs.rs.
    CratesIo,
    Osv,
    Audit,
}

impl Resource {
    pub fn key(self) -> String {
        match self {
            Resource::Mirror(project_id) => format!("mirror:{}", project_id.raw()),
            Resource::Discovery => "discovery".to_owned(),
            Resource::Npm => "npm".to_owned(),
            Resource::CratesIo => "crates.io".to_owned(),
            Resource::Osv => "osv".to_owned(),
            Resource::Audit => "audit".to_owned(),
        }
    }

    pub fn capacity(self) -> usize {
        match self {
            Resource::Mirror(_) => 1,
            Resource::Discovery => DISCOVERY,
            Resource::Npm => NPM,
            Resource::CratesIo => CRATES_IO,
            Resource::Osv => OSV,
            Resource::Audit => AUDIT,
        }
    }

    /// Makes every job that needs this resource wait until `until`. A block already reaching
    /// further stays: two jobs that hit one limit must not shorten each other's wait.
    pub async fn block(
        self,
        database: &Database,
        until: Timestamp,
        reason: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO resource_blocks (resource, blocked_until, reason) VALUES (?, ?, ?) \
             ON CONFLICT (resource) DO UPDATE SET \
                 reason = excluded.reason, \
                 blocked_until = MAX(blocked_until, excluded.blocked_until)",
        )
        .bind(self.key())
        .bind(until.to_string())
        .bind(reason)
        .execute(&database.pool)
        .await?;

        Ok(())
    }

    pub async fn clear_expired_blocks(database: &Database) -> Result<u64, DatabaseError> {
        let cleared = sqlx::query("DELETE FROM resource_blocks WHERE blocked_until <= ?")
            .bind(Timestamp::now().to_string())
            .execute(&database.pool)
            .await?;

        Ok(cleared.rows_affected())
    }
}
