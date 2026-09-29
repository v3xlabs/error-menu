//! What a package registry says about one published version, and the jobs that ask.
//!
//! No analyzer reads a registry. A scan must not wait on somebody else's server, and the
//! answer is the same answer for every project that locks the same version, so each version
//! is fetched by its own job and the cache is keyed by the coordinate alone.

mod cargo;
mod npm;
mod osv;

use std::collections::BTreeMap;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::analysis::finding::fingerprint::{Components, Fingerprint};
use crate::analysis::{NewRun, RunStatus};
use crate::app::AppState;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::outbound;
use crate::prelude::*;
use crate::worker::queue::{Job, Priority, Work};
use crate::worker::resource::Resource;

/// The run identifiers the audits record under. They are not selectable analyzers: nothing
/// schedules them except the audit job, so they are absent from `DEFAULT_ANALYZERS` and
/// `runner::evaluate` has no arm for them.
pub const CHECKSUM_AUDIT: &str = "checksum-audit";
pub const PACKAGE_AUDIT: &str = "package-audit";

const CHECKSUM_MISMATCH: &str = "checksum-mismatch";
const KNOWN_ADVISORY: &str = "known-advisory";
const WITHDRAWN: &str = "withdrawn-version";
const HEAVY: &str = "heavy-package";

/// What a reviewer should hear about before a package lands. The package's own published
/// size, not its install tree: each dependency arrives as its own coordinate and is weighed
/// there, so counting the tree would report one heavy dependency once per dependent.
const HEAVY_BYTES: u64 = 5_000_000;

/// How long an answer is trusted. A published version's size and checksum never change, but
/// its download count, its advisories and its deprecation notice do, so the row expires as
/// a whole.
const KNOWN_TTL: SignedDuration = SignedDuration::from_hours(24);
const ABSENT_TTL: SignedDuration = SignedDuration::from_hours(6);
const FAILED_TTL: SignedDuration = SignedDuration::from_mins(15);

/// How long a registry that answered 429 without saying how long is left alone.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(60);

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
    pub license: Option<String>,
    pub documentation: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub checksum: Option<String>,
    /// The yank state for Cargo and the deprecation notice for npm. They are one fact to a
    /// reader: the publisher has taken this back.
    pub withdrawn: Option<String>,
    /// Published against this exact version. A known row with none is an answer: the
    /// advisory read is required, so a failed one fails the row.
    pub advisories: Vec<Advisory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    pub id: String,
    /// CVE, GHSA and RUSTSEC identifiers for the same problem.
    pub aliases: Vec<String>,
    pub severity: AdvisorySeverity,
    pub summary: Option<String>,
    pub fixed_in: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvisorySeverity {
    Critical,
    High,
    Moderate,
    Low,
    /// A RustSec notice that is not a vulnerability: unmaintained, unsound, or a notice.
    Informational,
    /// Published without a rating.
    Unrated,
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

impl Coordinate {
    /// The identity a job keeps for this version, `npm:@scope/name@1.2.3`. A version never
    /// holds an `@`, so the last one ends the name even for a scoped package.
    pub fn subject(&self) -> String {
        format!("{}:{}@{}", self.ecosystem.stored(), self.name, self.version)
    }

    pub fn from_subject(subject: &str) -> Result<Self, DatabaseError> {
        let unreadable = || DatabaseError::Unreadable {
            field: "job subject",
            value: subject.to_owned(),
        };
        let (ecosystem, package) = subject.split_once(':').ok_or_else(unreadable)?;
        let (name, version) = package.rsplit_once('@').ok_or_else(unreadable)?;

        Ok(Self {
            ecosystem: Ecosystem::parse_stored(ecosystem).ok_or_else(unreadable)?,
            name: name.to_owned(),
            version: version.to_owned(),
        })
    }
}

impl DecodeRow for Coordinate {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        Ok(Coordinate {
            ecosystem: Ecosystem::read(row, "ecosystem")?,
            name: row.try_get("package_name")?,
            version: row.try_get("package_version")?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("outbound client: {0}")]
    Client(#[from] reqwest::Error),
    /// Nothing was stored: the read never happened, and every job that needs the same
    /// registry should wait with this one.
    #[error("{} asked us to wait until {until}", resource.key())]
    RateLimited {
        resource: Resource,
        until: Timestamp,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("not published")]
    NotFound,
    #[error("{} asked us to wait until {until}", resource.key())]
    RateLimited {
        resource: Resource,
        until: Timestamp,
    },
    #[error("{0}")]
    Unavailable(String),
}

/// What one fetch left in the cache.
#[derive(Debug)]
pub enum Fetch {
    /// Known or absent: an answer the audit can use.
    Answered,
    /// The failure is cached until `retry_at`, so asking again before then reads nothing.
    Failed { retry_at: Timestamp },
}

/// How many unanswered packages one sweep queues. The sweep repeats, so a backlog larger
/// than this drains over several.
const SWEEP_LIMIT: i64 = 1000;

/// Reads one version from its registry unless the cache already answers it, which it does
/// when another project's job got there first.
pub async fn fetch(state: &AppState, coordinate: &Coordinate) -> Result<Fetch, RegistryError> {
    if PackageFacts::is_fresh(&state.database, coordinate).await? {
        return Ok(Fetch::Answered);
    }

    let client = outbound::client()?;
    let read = match coordinate.ecosystem {
        Ecosystem::Cargo => cargo::read(&client, &coordinate.name, &coordinate.version).await,
        Ecosystem::Npm => npm::read(&client, &coordinate.name, &coordinate.version).await,
        // A flake input is a repository at a revision. No registry publishes it, and the
        // link it gets is built from the origin alone, so it is never queued.
        Ecosystem::Nix => return Ok(Fetch::Answered),
    };

    let facts = match read {
        Ok(facts) => facts,
        Err(ReadError::RateLimited { resource, until }) => {
            return Err(RegistryError::RateLimited { resource, until });
        }
        Err(ReadError::NotFound) => PackageFacts::empty(coordinate.clone(), FactsStatus::Absent),
        Err(ReadError::Unavailable(reason)) => {
            tracing::warn!(
                package = %coordinate.name,
                version = %coordinate.version,
                %reason,
                "registry read failed"
            );
            PackageFacts::empty(coordinate.clone(), FactsStatus::Failed)
        }
    };
    PackageFacts::upsert(&state.database, &facts).await?;

    Ok(match facts.status {
        FactsStatus::Known | FactsStatus::Absent => Fetch::Answered,
        FactsStatus::Failed => Fetch::Failed {
            retry_at: facts.expires_at,
        },
    })
}

/// Queues what an analysis needs from the registries: a fetch for every public-registry
/// version in the snapshot the cache cannot answer, and the audit that reads them. The audit
/// is queued even when every answer is cached, because then nothing else would queue it.
pub async fn queue_for_snapshot(
    database: &Database,
    project_id: Id<Project>,
    snapshot_id: Id<Snapshot>,
) -> Result<(), DatabaseError> {
    let mut work: Vec<Work> = PackageFacts::stale_in_snapshot(database, snapshot_id)
        .await?
        .into_iter()
        .map(Work::PackageFacts)
        .collect();
    work.push(Work::PackageAudit { project_id });

    Job::enqueue_all(database, &work, Priority::Requested).await?;

    Ok(())
}

/// Queues an audit for every project with a snapshot that names this version and has not
/// been audited yet. The audit decides for itself whether the snapshot is ready.
pub async fn queue_audits(
    database: &Database,
    coordinate: &Coordinate,
) -> Result<(), DatabaseError> {
    let work: Vec<Work> = PackageFacts::unaudited_projects(database, coordinate)
        .await?
        .into_iter()
        .map(|project_id| Work::PackageAudit { project_id })
        .collect();

    Job::enqueue_all(database, &work, Priority::Requested).await?;

    Ok(())
}

/// Queues a fetch for versions that were never answered, or whose failure has expired. An
/// analysis queues its own versions when it finishes, so this only catches what that missed:
/// a cache emptied by a migration, or a fetch that ran out of attempts.
pub async fn sweep(database: &Database) -> Result<usize, DatabaseError> {
    let work: Vec<Work> = PackageFacts::unanswered(database, SWEEP_LIMIT)
        .await?
        .into_iter()
        .map(Work::PackageFacts)
        .collect();

    Job::enqueue_all(database, &work, Priority::Backfill).await
}

/// Records what the facts say about every snapshot whose registry answers are all in. The
/// facts arrive after the analysis finished, so each audit is its own run on the snapshot
/// and no finished run is written to twice.
pub async fn audit(database: &Database, project_id: Id<Project>) -> Result<(), DatabaseError> {
    for snapshot_id in Snapshot::awaiting_audit(database, project_id, CHECKSUM_AUDIT).await? {
        let findings: Vec<NewFinding> = PackageFacts::checksum_mismatches(database, snapshot_id)
            .await?
            .into_iter()
            .map(mismatch_finding)
            .collect();

        record_audit(database, snapshot_id, CHECKSUM_AUDIT, &findings).await?;
    }

    for snapshot_id in Snapshot::awaiting_audit(database, project_id, PACKAGE_AUDIT).await? {
        let mut findings = Vec::new();
        for arrival in PackageFacts::arrivals(database, snapshot_id).await? {
            findings.extend(arrival_findings(arrival));
        }

        record_audit(database, snapshot_id, PACKAGE_AUDIT, &findings).await?;
    }

    Ok(())
}

async fn record_audit(
    database: &Database,
    snapshot_id: Id<Snapshot>,
    analyzer: &str,
    findings: &[NewFinding],
) -> Result<(), DatabaseError> {
    Run::record(
        database,
        NewRun {
            snapshot_id,
            analyzer,
            status: RunStatus::Succeeded,
            compared_against: None,
            findings,
            signals: &[],
        },
    )
    .await?;

    Ok(())
}

#[derive(Debug)]
pub struct ChecksumMismatch {
    pub location: Location,
    pub claimed: String,
    pub published: String,
}

fn mismatch_finding(mismatch: ChecksumMismatch) -> NewFinding {
    let (name, version) = coordinate_of(&mismatch.location);
    let title = format!("{name} {version} does not match the published checksum");
    let detail = format!(
        "the lockfile pins {name} {version} to {} and the registry published {}",
        mismatch.claimed, mismatch.published
    );

    audit_finding(
        CHECKSUM_AUDIT,
        CHECKSUM_MISMATCH,
        mismatch.location,
        Severity::Critical,
        title,
        detail,
    )
}

/// A package version this snapshot brings in, with what its registry said about it.
#[derive(Debug)]
struct Arrival {
    location: Location,
    bytes: Option<u64>,
    withdrawn: Option<String>,
    advisories: Vec<Advisory>,
}

fn arrival_findings(arrival: Arrival) -> Vec<NewFinding> {
    let (name, version) = coordinate_of(&arrival.location);
    let mut findings = Vec::new();

    for advisory in &arrival.advisories {
        let mut detail = format!("{name} {version} is affected by {}", advisory.id);
        if let Some(summary) = &advisory.summary {
            detail.push_str(&format!(": {summary}"));
        }
        if !advisory.aliases.is_empty() {
            detail.push_str(&format!(". Also known as {}", advisory.aliases.join(", ")));
        }
        match &advisory.fixed_in {
            Some(fixed) => detail.push_str(&format!(". Fixed in {fixed}")),
            None => detail.push_str(". No fixed version is published"),
        }

        findings.push(audit_finding(
            PACKAGE_AUDIT,
            KNOWN_ADVISORY,
            arrival.location.clone(),
            advisory_severity(advisory.severity),
            format!("{name} {version} is affected by {}", advisory.id),
            detail,
        ));
    }

    if let Some(notice) = &arrival.withdrawn {
        findings.push(audit_finding(
            PACKAGE_AUDIT,
            WITHDRAWN,
            arrival.location.clone(),
            Severity::Medium,
            format!("{name} {version} was withdrawn by its publisher"),
            format!("{name} {version} was withdrawn: {notice}"),
        ));
    }

    if let Some(bytes) = arrival.bytes.filter(|bytes| *bytes >= HEAVY_BYTES) {
        let megabytes = bytes as f64 / 1_000_000.0;
        findings.push(audit_finding(
            PACKAGE_AUDIT,
            HEAVY,
            arrival.location.clone(),
            Severity::Medium,
            format!("{name} {version} weighs {megabytes:.1} MB"),
            format!("{name} {version} is {megabytes:.1} MB as published, before its dependencies"),
        ));
    }

    findings
}

/// An unmaintained crate is worth reading about and not worth stopping for. An advisory
/// nobody rated could be anything, so it asks for attention.
fn advisory_severity(severity: AdvisorySeverity) -> Severity {
    match severity {
        AdvisorySeverity::Critical => Severity::Critical,
        AdvisorySeverity::High => Severity::High,
        AdvisorySeverity::Moderate | AdvisorySeverity::Unrated => Severity::Medium,
        AdvisorySeverity::Low | AdvisorySeverity::Informational => Severity::Low,
    }
}

fn coordinate_of(location: &Location) -> (String, String) {
    match location {
        Location::Package { name, version, .. } => (name.clone(), version.clone()),
        Location::File { path, .. } => (path.as_str().to_owned(), String::new()),
    }
}

fn audit_finding(
    analyzer: &str,
    rule: &str,
    location: Location,
    severity: Severity,
    title: String,
    detail: String,
) -> NewFinding {
    NewFinding {
        movement: None,
        fingerprint: Fingerprint::compute(&Components {
            analyzer,
            rule,
            location: &location,
            title: &title,
            occurrence: 0,
        }),
        location,
        severity,
        confidence: Confidence::new(1.0).expect("one is in range"),
        attribution: Attribution::Introduced,
        title,
        detail,
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
            license: None,
            documentation: None,
            repository: None,
            homepage: None,
            checksum: None,
            withdrawn: None,
            advisories: Vec::new(),
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

    async fn is_fresh(database: &Database, coordinate: &Coordinate) -> Result<bool, DatabaseError> {
        let fresh = sqlx::query(
            "SELECT 1 FROM package_facts \
             WHERE ecosystem = ? AND name = ? AND version = ? AND expires_at > ?",
        )
        .bind(coordinate.ecosystem.stored())
        .bind(&coordinate.name)
        .bind(&coordinate.version)
        .bind(Timestamp::now().to_string())
        .fetch_optional(&database.pool)
        .await?;

        Ok(fresh.is_some())
    }

    /// The versions this snapshot names that the cache cannot answer. Only a package the
    /// lockfile resolved from the public registry is asked about: a workspace member, a git
    /// pin or a private registry package can share a public name and version and still be
    /// different bytes.
    async fn stale_in_snapshot(
        database: &Database,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<Coordinate>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.ecosystem, f.package_name, f.package_version \
             FROM findings f \
             JOIN runs r ON r.id = f.run_id \
             LEFT JOIN package_facts p ON p.ecosystem = f.ecosystem \
                  AND p.name = f.package_name AND p.version = f.package_version \
             WHERE r.snapshot_id = ? AND f.location_kind = 'package' \
               AND f.origin_kind = 'public_registry' AND f.ecosystem <> 'nix' \
               AND (p.ecosystem IS NULL OR p.expires_at <= ?)",
        )
        .bind(snapshot_id.raw())
        .bind(Timestamp::now().to_string())
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(Coordinate::decode_row).collect()
    }

    /// Versions any snapshot names that have no answer: never fetched, or failed and due
    /// again. An expired known answer is still an answer, so it waits for an analysis that
    /// names it to ask for it fresh.
    async fn unanswered(database: &Database, limit: i64) -> Result<Vec<Coordinate>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.ecosystem, f.package_name, f.package_version \
             FROM findings f \
             LEFT JOIN package_facts p ON p.ecosystem = f.ecosystem \
                  AND p.name = f.package_name AND p.version = f.package_version \
             WHERE f.location_kind = 'package' AND f.origin_kind = 'public_registry' \
               AND f.ecosystem <> 'nix' \
               AND (p.ecosystem IS NULL OR (p.status = 'failed' AND p.expires_at <= ?)) \
             LIMIT ?",
        )
        .bind(Timestamp::now().to_string())
        .bind(limit)
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(Coordinate::decode_row).collect()
    }

    /// Projects with a snapshot that names this version, lacks one of the audits, and has
    /// no other version still unanswered. An audit before that would find the snapshot not
    /// ready and do nothing, once per fetch.
    async fn unaudited_projects(
        database: &Database,
        coordinate: &Coordinate,
    ) -> Result<Vec<Id<Project>>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT sub.project_id FROM findings f \
             JOIN runs r ON r.id = f.run_id \
             JOIN snapshots s ON s.id = r.snapshot_id \
             JOIN subjects sub ON sub.id = s.subject_id \
             WHERE f.ecosystem = ? AND f.package_name = ? AND f.package_version = ? \
               AND f.origin_kind = 'public_registry' \
               AND (NOT EXISTS (SELECT 1 FROM runs a WHERE a.snapshot_id = s.id AND a.analyzer = ?) \
                 OR NOT EXISTS (SELECT 1 FROM runs a WHERE a.snapshot_id = s.id AND a.analyzer = ?)) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM runs ur JOIN findings u ON u.run_id = ur.id \
                   WHERE ur.snapshot_id = s.id AND u.location_kind = 'package' \
                     AND u.origin_kind = 'public_registry' AND u.ecosystem <> 'nix' \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM package_facts p \
                         WHERE p.ecosystem = u.ecosystem AND p.name = u.package_name \
                           AND p.version = u.package_version \
                           AND p.status IN ('known', 'absent') \
                     ) \
               )",
        )
        .bind(coordinate.ecosystem.stored())
        .bind(&coordinate.name)
        .bind(&coordinate.version)
        .bind(CHECKSUM_AUDIT)
        .bind(PACKAGE_AUDIT)
        .fetch_all(&database.pool)
        .await?;

        rows.iter()
            .map(|row| Ok(Id::from_raw(row.try_get("project_id")?)))
            .collect()
    }

    /// Every fact this project's findings can be joined to, in two reads, so a response
    /// that lists hundreds of findings makes two queries and not hundreds.
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

        let mut facts = rows
            .iter()
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
            .collect::<Result<BTreeMap<_, _>, DatabaseError>>()?;

        let advisories = sqlx::query(
            "SELECT DISTINCT a.* FROM package_advisories a \
             JOIN findings f ON f.ecosystem = a.ecosystem AND f.package_name = a.name \
                  AND f.package_version = a.version \
             JOIN runs r ON r.id = f.run_id \
             JOIN snapshots s ON s.id = r.snapshot_id \
             JOIN subjects sub ON sub.id = s.subject_id \
             WHERE sub.project_id = ? AND f.origin_kind = 'public_registry' \
             ORDER BY a.advisory_id",
        )
        .bind(project_id.raw())
        .fetch_all(&database.pool)
        .await?;

        for row in &advisories {
            let key: (&'static str, String, String) = (
                Ecosystem::read(row, "ecosystem")?.stored(),
                row.try_get("name")?,
                row.try_get("version")?,
            );
            if let Some(known) = facts.get_mut(&key) {
                known.advisories.push(Advisory::decode(row)?);
            }
        }

        Ok(facts)
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

    /// The public-registry versions this snapshot brings in: added, or moved to. A removed
    /// version leaves with its problems, and an unchanged one was reviewed when it arrived.
    async fn arrivals(
        database: &Database,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<Arrival>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.location_kind, f.file_path, f.line_start, f.line_end, \
                    f.ecosystem, f.package_name, f.package_version, f.origin_kind, \
                    f.origin_ref, f.package_integrity, \
                    p.size_bytes, p.withdrawn \
             FROM findings f \
             JOIN runs r ON r.id = f.run_id \
             JOIN package_facts p ON p.ecosystem = f.ecosystem AND p.name = f.package_name \
                  AND p.version = f.package_version \
             WHERE r.snapshot_id = ? AND p.status = 'known' \
               AND f.origin_kind = 'public_registry' \
               AND f.movement IN ('added', 'upgraded', 'downgraded', 'changed')",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&database.pool)
        .await?;

        let mut arrivals = Vec::with_capacity(rows.len());
        for row in &rows {
            let location = Location::decode_row(row)?;
            let advisories = match &location {
                Location::Package {
                    ecosystem,
                    name,
                    version,
                    ..
                } => Advisory::for_coordinate(database, *ecosystem, name, version).await?,
                Location::File { .. } => Vec::new(),
            };

            arrivals.push(Arrival {
                location,
                bytes: row
                    .try_get::<Option<i64>, _>("size_bytes")?
                    .map(|v| v as u64),
                withdrawn: row.try_get("withdrawn")?,
                advisories,
            });
        }

        Ok(arrivals)
    }

    async fn upsert(database: &Database, facts: &PackageFacts) -> Result<(), DatabaseError> {
        let mut transaction = database.write().await?;

        sqlx::query(
            "INSERT OR REPLACE INTO package_facts \
             (ecosystem, name, version, status, fetched_at, expires_at, size_bytes, \
              install_bytes, dependency_count, downloads_week, license, documentation, \
              repository, homepage, checksum, withdrawn) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&facts.license)
        .bind(&facts.documentation)
        .bind(&facts.repository)
        .bind(&facts.homepage)
        .bind(&facts.checksum)
        .bind(&facts.withdrawn)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "DELETE FROM package_advisories WHERE ecosystem = ? AND name = ? AND version = ?",
        )
        .bind(facts.ecosystem.stored())
        .bind(&facts.name)
        .bind(&facts.version)
        .execute(&mut *transaction)
        .await?;

        for advisory in &facts.advisories {
            sqlx::query(
                "INSERT OR REPLACE INTO package_advisories \
                 (ecosystem, name, version, advisory_id, aliases, severity, summary, fixed_in) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(facts.ecosystem.stored())
            .bind(&facts.name)
            .bind(&facts.version)
            .bind(&advisory.id)
            .bind(advisory.aliases.join(" "))
            .bind(advisory.severity.stored())
            .bind(&advisory.summary)
            .bind(&advisory.fixed_in)
            .execute(&mut *transaction)
            .await?;
        }

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
            license: row.try_get("license")?,
            documentation: row.try_get("documentation")?,
            repository: row.try_get("repository")?,
            homepage: row.try_get("homepage")?,
            checksum: row.try_get("checksum")?,
            withdrawn: row.try_get("withdrawn")?,
            advisories: Vec::new(),
        })
    }
}

impl Advisory {
    async fn for_coordinate(
        database: &Database,
        ecosystem: Ecosystem,
        name: &str,
        version: &str,
    ) -> Result<Vec<Advisory>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT * FROM package_advisories \
             WHERE ecosystem = ? AND name = ? AND version = ? ORDER BY advisory_id",
        )
        .bind(ecosystem.stored())
        .bind(name)
        .bind(version)
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(Advisory::decode).collect()
    }

    fn decode(row: &SqliteRow) -> Result<Advisory, DatabaseError> {
        Ok(Advisory {
            id: row.try_get("advisory_id")?,
            aliases: row
                .try_get::<&str, _>("aliases")?
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            severity: AdvisorySeverity::read(row, "severity")?,
            summary: row.try_get("summary")?,
            fixed_in: row.try_get("fixed_in")?,
        })
    }
}

impl AdvisorySeverity {
    /// The words GitHub, npm and OSV rate with, in whichever case they arrive.
    fn from_rating(rating: &str) -> Self {
        match rating.to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "moderate" | "medium" => Self::Moderate,
            "low" => Self::Low,
            _ => Self::Unrated,
        }
    }
}

impl StoredAs for AdvisorySeverity {
    fn stored(&self) -> &'static str {
        match self {
            AdvisorySeverity::Critical => "critical",
            AdvisorySeverity::High => "high",
            AdvisorySeverity::Moderate => "moderate",
            AdvisorySeverity::Low => "low",
            AdvisorySeverity::Informational => "informational",
            AdvisorySeverity::Unrated => "unrated",
        }
    }
}

impl FromStored for AdvisorySeverity {
    const FIELD: &'static str = "severity";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "critical" => Some(AdvisorySeverity::Critical),
            "high" => Some(AdvisorySeverity::High),
            "moderate" => Some(AdvisorySeverity::Moderate),
            "low" => Some(AdvisorySeverity::Low),
            "informational" => Some(AdvisorySeverity::Informational),
            "unrated" => Some(AdvisorySeverity::Unrated),
            _ => None,
        }
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

/// One bounded read of a public JSON endpoint. A 404 is an answer about the package, a 429
/// is a wait on `resource`, and every other refusal is an answer about the registry.
pub(crate) async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    resource: Resource,
    url: &str,
) -> Result<T, ReadError> {
    let parsed = checked(url)?;
    receive(resource, url, client.get(parsed)).await
}

/// A query sent as a JSON body, for an API that takes its question that way.
pub(crate) async fn post<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    resource: Resource,
    url: &str,
    body: &impl serde::Serialize,
) -> Result<T, ReadError> {
    let parsed = checked(url)?;
    receive(resource, url, client.post(parsed).json(body)).await
}

fn checked(url: &str) -> Result<url::Url, ReadError> {
    let parsed = url
        .parse()
        .map_err(|error| ReadError::Unavailable(format!("{url} is not a url: {error}")))?;
    outbound::validate_url(&parsed).map_err(|error| ReadError::Unavailable(error.to_string()))?;

    Ok(parsed)
}

async fn receive<T: serde::de::DeserializeOwned>(
    resource: Resource,
    url: &str,
    request: reqwest::RequestBuilder,
) -> Result<T, ReadError> {
    let mut response = request
        .send()
        .await
        .map_err(|error| ReadError::Unavailable(error.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ReadError::NotFound);
    }

    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // Only the delay form of Retry-After: a registry that sends a date is rare enough
        // that the default pause covers it.
        let pause = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map_or(RATE_LIMIT_PAUSE, Duration::from_secs);
        return Err(ReadError::RateLimited {
            resource,
            until: Timestamp::now() + pause,
        });
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
