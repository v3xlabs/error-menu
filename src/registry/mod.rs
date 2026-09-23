//! What a package registry says about one published version, and the job that asks.
//!
//! No analyzer reads a registry. A scan must not wait on somebody else's server, and the
//! answer is the same answer for every project that locks the same version, so the fetch
//! runs on its own job and the cache is keyed by the coordinate alone.

mod cargo;
mod npm;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::analysis::finding::fingerprint::{Components, Fingerprint};
use crate::analysis::{NewRun, RunStatus};
use crate::app::AppState;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::outbound;
use crate::prelude::*;
use crate::worker::queue::LEASE;

/// The run identifier the checksum audit records under. It is not a selectable analyzer:
/// nothing schedules it except the facts job, so it is absent from `DEFAULT_ANALYZERS` and
/// `runner::evaluate` has no arm for it.
pub const AUDIT: &str = "checksum-audit";

const CHECKSUM_MISMATCH: &str = "checksum-mismatch";

/// How long an answer is trusted. A published version's size and checksum never change, but
/// its download count and its deprecation notice do, so the row expires as a whole.
const KNOWN_TTL: SignedDuration = SignedDuration::from_hours(24);
const ABSENT_TTL: SignedDuration = SignedDuration::from_hours(6);
const FAILED_TTL: SignedDuration = SignedDuration::from_mins(15);

/// How many coordinates one job reads, and how long it may spend reading them. A first
/// scan of a large repository can reference thousands, and a job that tried them all would
/// outlive its lease. Either limit queues the job again rather than leaving the rest unread.
const PER_JOB: usize = 200;

/// Half the lease, because the check runs between coordinates and one coordinate can spend
/// several request timeouts past it.
const BUDGET: Duration = Duration::from_secs(LEASE.as_secs() / 2);

/// The response body cap. Every endpoint this module reads answers in kilobytes; a
/// megabyte means the URL is not what we think it is.
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// One published version, as the registry describes it. Every field is optional because no
/// registry answers all of them, and a field nobody published is not a field we failed to
/// read.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageFacts {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
    pub status: FactsStatus,
    pub fetched_at: Timestamp,
    pub expires_at: Timestamp,
    pub size_bytes: Option<u64>,
    pub install_bytes: Option<u64>,
    pub dependency_count: Option<u32>,
    pub downloads_week: Option<u64>,
    pub vulnerabilities: Option<u32>,
    pub vulnerabilities_high: Option<u32>,
    pub license: Option<String>,
    pub documentation: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub checksum: Option<String>,
    /// The yank state for Cargo and the deprecation notice for npm. They are one fact to a
    /// reader: the publisher has taken this back.
    pub withdrawn: Option<String>,
}

/// `Absent` is an answer, not a gap: a private package that the public registry has never
/// heard of must not look like a fetch that has not happened yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactsStatus {
    Known,
    Absent,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coordinate {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("outbound client: {0}")]
    Client(#[from] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("not published")]
    NotFound,
    #[error("{0}")]
    Unavailable(String),
}

/// What one job left behind, so the queue knows whether to ask again and when.
#[derive(Debug)]
pub enum Fill {
    Complete,
    /// Coordinates are left that this job had no room or time for.
    Unfinished,
    /// A failed read is cached until `retry_at`, so asking again before then reads nothing.
    Failed {
        reads: usize,
        retry_at: Timestamp,
    },
}

/// Fills every coordinate this project's findings reference that the cache does not hold or
/// holds stale. The cache is global, so a coordinate another project already paid for is
/// skipped here without a request.
pub async fn fill(state: &AppState, project_id: Id<Project>) -> Result<Fill, RegistryError> {
    let client = outbound::client()?;
    let started = Instant::now();
    let mut failed = None;

    for coordinate in PackageFacts::stale_for_project(&state.database, project_id, PER_JOB).await? {
        if started.elapsed() >= BUDGET {
            break;
        }

        let read = match coordinate.ecosystem {
            Ecosystem::Cargo => cargo::read(&client, &coordinate.name, &coordinate.version).await,
            Ecosystem::Npm => npm::read(&client, &coordinate.name, &coordinate.version).await,
            // A flake input is a repository at a revision. No registry publishes it, and
            // the link it gets is built from the origin alone.
            Ecosystem::Nix => continue,
        };

        if let Err(ReadError::Unavailable(reason)) = &read {
            tracing::warn!(
                package = %coordinate.name,
                version = %coordinate.version,
                %reason,
                "registry read failed"
            );
        }

        let facts = resolve(coordinate, read);
        if facts.status == FactsStatus::Failed {
            let reads = failed.map_or(0, |(reads, _)| reads);
            failed = Some((reads + 1, facts.expires_at));
        }

        PackageFacts::upsert(&state.database, &facts).await?;
    }

    audit_checksums(state, project_id).await?;

    if let Some((reads, retry_at)) = failed {
        return Ok(Fill::Failed { reads, retry_at });
    }

    let unfinished = !PackageFacts::stale_for_project(&state.database, project_id, 1)
        .await?
        .is_empty();

    Ok(if unfinished {
        Fill::Unfinished
    } else {
        Fill::Complete
    })
}

/// Compares what each lockfile claimed against what the publisher published. A coordinate
/// with no cached checksum is not compared and not reported: a missing answer is neither a
/// match nor a mismatch.
async fn audit_checksums(state: &AppState, project_id: Id<Project>) -> Result<(), RegistryError> {
    for snapshot_id in Snapshot::awaiting_audit(&state.database, project_id, AUDIT).await? {
        let findings: Vec<NewFinding> =
            PackageFacts::checksum_mismatches(&state.database, snapshot_id)
                .await?
                .into_iter()
                .map(mismatch_finding)
                .collect();

        Run::record(
            &state.database,
            NewRun {
                snapshot_id,
                analyzer: AUDIT,
                status: RunStatus::Succeeded,
                compared_against: None,
                findings: &findings,
                signals: &[],
            },
        )
        .await?;
    }

    Ok(())
}

#[derive(Debug)]
pub struct ChecksumMismatch {
    pub location: Location,
    pub claimed: String,
    pub published: String,
}

fn mismatch_finding(mismatch: ChecksumMismatch) -> NewFinding {
    let (name, version) = match &mismatch.location {
        Location::Package { name, version, .. } => (name.clone(), version.clone()),
        Location::File { path, .. } => (path.as_str().to_owned(), String::new()),
    };
    let title = format!("{name} {version} does not match the published checksum");
    let detail = format!(
        "the lockfile pins {name} {version} to {} and the registry published {}",
        mismatch.claimed, mismatch.published
    );

    NewFinding {
        movement: None,
        fingerprint: Fingerprint::compute(&Components {
            analyzer: AUDIT,
            rule: CHECKSUM_MISMATCH,
            location: &mismatch.location,
            title: &title,
            occurrence: 0,
        }),
        location: mismatch.location,
        severity: Severity::Critical,
        confidence: Confidence::new(1.0).expect("one is in range"),
        attribution: Attribution::Introduced,
        title,
        detail,
    }
}

fn resolve(coordinate: Coordinate, read: Result<PackageFacts, ReadError>) -> PackageFacts {
    match read {
        Ok(facts) => facts,
        Err(ReadError::NotFound) => PackageFacts::empty(coordinate, FactsStatus::Absent),
        Err(ReadError::Unavailable(_)) => PackageFacts::empty(coordinate, FactsStatus::Failed),
    }
}

impl PackageFacts {
    pub(crate) fn empty(coordinate: Coordinate, status: FactsStatus) -> Self {
        let fetched_at = Timestamp::now();
        let ttl = match status {
            FactsStatus::Known => KNOWN_TTL,
            FactsStatus::Absent => ABSENT_TTL,
            FactsStatus::Failed => FAILED_TTL,
        };

        Self {
            ecosystem: coordinate.ecosystem,
            name: coordinate.name,
            version: coordinate.version,
            status,
            fetched_at,
            expires_at: fetched_at + ttl,
            size_bytes: None,
            install_bytes: None,
            dependency_count: None,
            downloads_week: None,
            vulnerabilities: None,
            vulnerabilities_high: None,
            license: None,
            documentation: None,
            repository: None,
            homepage: None,
            checksum: None,
            withdrawn: None,
        }
    }

    pub(crate) fn known(ecosystem: Ecosystem, name: &str, version: &str) -> Self {
        Self::empty(
            Coordinate {
                ecosystem,
                name: name.to_owned(),
                version: version.to_owned(),
            },
            FactsStatus::Known,
        )
    }

    /// The coordinates this project's findings name that the cache cannot answer. Only a
    /// package the lockfile resolved from the public registry is asked about: a workspace
    /// member, a git pin or a private registry package can share a public name and version
    /// and still be different bytes.
    async fn stale_for_project(
        database: &Database,
        project_id: Id<Project>,
        limit: usize,
    ) -> Result<Vec<Coordinate>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.ecosystem, f.package_name, f.package_version \
             FROM findings f \
             JOIN runs r ON r.id = f.run_id \
             JOIN snapshots s ON s.id = r.snapshot_id \
             JOIN subjects sub ON sub.id = s.subject_id \
             LEFT JOIN package_facts p ON p.ecosystem = f.ecosystem \
                  AND p.name = f.package_name AND p.version = f.package_version \
             WHERE sub.project_id = ? AND f.location_kind = 'package' \
               AND f.origin_kind = 'public_registry' AND f.ecosystem <> 'nix' \
               AND (p.ecosystem IS NULL OR p.expires_at <= ?) \
             ORDER BY f.package_name LIMIT ?",
        )
        .bind(project_id.raw())
        .bind(Timestamp::now().to_string())
        .bind(limit as i64)
        .fetch_all(&database.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Coordinate {
                    ecosystem: Ecosystem::read(&row, "ecosystem")?,
                    name: row.try_get("package_name")?,
                    version: row.try_get("package_version")?,
                })
            })
            .collect()
    }

    /// Every fact this project's findings can be joined to, in one read, so a response
    /// that lists hundreds of findings makes one query and not hundreds.
    pub async fn for_project(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<BTreeMap<(&'static str, String, String), PackageFacts>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT p.* FROM package_facts p \
             JOIN findings f ON f.ecosystem = p.ecosystem AND f.package_name = p.name \
                  AND f.package_version = p.version \
             JOIN runs r ON r.id = f.run_id \
             JOIN snapshots s ON s.id = r.snapshot_id \
             JOIN subjects sub ON sub.id = s.subject_id \
             WHERE sub.project_id = ? AND p.status = 'known' \
               AND f.origin_kind = 'public_registry'",
        )
        .bind(project_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.iter()
            .map(|row| {
                let facts = PackageFacts::decode(row)?;
                Ok((
                    (
                        facts.ecosystem.stored(),
                        facts.name.clone(),
                        facts.version.clone(),
                    ),
                    facts,
                ))
            })
            .collect()
    }

    async fn checksum_mismatches(
        database: &Database,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<ChecksumMismatch>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.location_kind, f.file_path, f.line_start, f.line_end, \
                    f.ecosystem, f.package_name, f.package_version, f.origin_kind, \
                    f.origin_ref, f.package_integrity, p.checksum \
             FROM findings f \
             JOIN runs r ON r.id = f.run_id \
             JOIN package_facts p ON p.ecosystem = f.ecosystem AND p.name = f.package_name \
                  AND p.version = f.package_version \
             WHERE r.snapshot_id = ? AND p.checksum IS NOT NULL \
               AND f.origin_kind = 'public_registry' \
               AND f.package_integrity IS NOT NULL AND f.package_integrity <> p.checksum",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.iter()
            .map(|row| {
                Ok(ChecksumMismatch {
                    location: Location::decode_row(row)?,
                    claimed: row.try_get("package_integrity")?,
                    published: row.try_get("checksum")?,
                })
            })
            .collect()
    }

    async fn upsert(database: &Database, facts: &PackageFacts) -> Result<(), DatabaseError> {
        let mut transaction = database.write().await?;

        sqlx::query(
            "INSERT OR REPLACE INTO package_facts \
             (ecosystem, name, version, status, fetched_at, expires_at, size_bytes, \
              install_bytes, dependency_count, downloads_week, vulnerabilities, \
              vulnerabilities_high, license, documentation, repository, homepage, checksum, \
              withdrawn) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(facts.ecosystem.stored())
        .bind(&facts.name)
        .bind(&facts.version)
        .bind(facts.status.stored())
        .bind(facts.fetched_at.to_string())
        .bind(facts.expires_at.to_string())
        .bind(facts.size_bytes.map(|value| value as i64))
        .bind(facts.install_bytes.map(|value| value as i64))
        .bind(facts.dependency_count.map(i64::from))
        .bind(facts.downloads_week.map(|value| value as i64))
        .bind(facts.vulnerabilities.map(i64::from))
        .bind(facts.vulnerabilities_high.map(i64::from))
        .bind(&facts.license)
        .bind(&facts.documentation)
        .bind(&facts.repository)
        .bind(&facts.homepage)
        .bind(&facts.checksum)
        .bind(&facts.withdrawn)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(())
    }

    fn decode(row: &SqliteRow) -> Result<PackageFacts, DatabaseError> {
        Ok(PackageFacts {
            ecosystem: Ecosystem::read(row, "ecosystem")?,
            name: row.try_get("name")?,
            version: row.try_get("version")?,
            status: FactsStatus::read(row, "status")?,
            fetched_at: Timestamp::read(row, "fetched_at")?,
            expires_at: Timestamp::read(row, "expires_at")?,
            size_bytes: row
                .try_get::<Option<i64>, _>("size_bytes")?
                .map(|v| v as u64),
            install_bytes: row
                .try_get::<Option<i64>, _>("install_bytes")?
                .map(|v| v as u64),
            dependency_count: row
                .try_get::<Option<i64>, _>("dependency_count")?
                .map(|v| v as u32),
            downloads_week: row
                .try_get::<Option<i64>, _>("downloads_week")?
                .map(|v| v as u64),
            vulnerabilities: row
                .try_get::<Option<i64>, _>("vulnerabilities")?
                .map(|v| v as u32),
            vulnerabilities_high: row
                .try_get::<Option<i64>, _>("vulnerabilities_high")?
                .map(|v| v as u32),
            license: row.try_get("license")?,
            documentation: row.try_get("documentation")?,
            repository: row.try_get("repository")?,
            homepage: row.try_get("homepage")?,
            checksum: row.try_get("checksum")?,
            withdrawn: row.try_get("withdrawn")?,
        })
    }
}

impl StoredAs for FactsStatus {
    fn stored(&self) -> &'static str {
        match self {
            FactsStatus::Known => "known",
            FactsStatus::Absent => "absent",
            FactsStatus::Failed => "failed",
        }
    }
}

impl FromStored for FactsStatus {
    const FIELD: &'static str = "status";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "known" => Some(FactsStatus::Known),
            "absent" => Some(FactsStatus::Absent),
            "failed" => Some(FactsStatus::Failed),
            _ => None,
        }
    }
}

/// One bounded read of a public JSON endpoint. A 404 is an answer about the package; every
/// other refusal is an answer about the registry.
pub(crate) async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, ReadError> {
    let parsed = url
        .parse()
        .map_err(|error| ReadError::Unavailable(format!("{url} is not a url: {error}")))?;
    outbound::validate_url(&parsed).map_err(|error| ReadError::Unavailable(error.to_string()))?;

    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| ReadError::Unavailable(error.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ReadError::NotFound);
    }

    if !response.status().is_success() {
        return Err(ReadError::Unavailable(format!(
            "{url} answered {}",
            response.status()
        )));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ReadError::Unavailable(error.to_string()))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ReadError::Unavailable(format!(
                "{url} answered more than {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }

    serde_json::from_slice(&body).map_err(|error| ReadError::Unavailable(error.to_string()))
}
