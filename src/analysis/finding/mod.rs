pub mod compare;
pub mod fingerprint;
pub mod issue;

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::analysis::Run;
use crate::analysis::confidence::Confidence;
use crate::analysis::confidence_from_stored;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::database::{Database, DatabaseError};
use crate::id::Id;
use crate::vcs::RepoPath;
use fingerprint::Fingerprint;
use issue::Issue;

/// One run saw this, here. Immutable, belongs to one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub movement: Option<VersionMovement>,
    pub id: Id<Finding>,
    pub run_id: Id<Run>,
    pub issue_id: Id<Issue>,
    pub fingerprint: Fingerprint,
    pub location: Location,
    pub severity: Severity,
    pub confidence: Confidence,
    pub attribution: Attribution,
    pub title: String,
    pub detail: String,
}

/// A finding as an analyzer produces it, before the database resolves its issue and gives
/// it an identity.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFinding {
    pub movement: Option<VersionMovement>,
    pub fingerprint: Fingerprint,
    pub location: Location,
    pub severity: Severity,
    pub confidence: Confidence,
    pub attribution: Attribution,
    pub title: String,
    pub detail: String,
}

/// Mandatory: a thing with no location cannot be fingerprinted, so it cannot be
/// deduplicated, so it is a signal and not a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Location {
    File {
        path: RepoPath,
        span: Option<LineSpan>,
    },
    /// A package coordinate rather than a lockfile line, so the finding survives the
    /// lockfile being regenerated. The path says which lockfile, because one repository
    /// can hold several.
    Package {
        path: RepoPath,
        ecosystem: Ecosystem,
        name: String,
        version: String,
        /// Absent for a finding recorded before origins were kept, which is why no link is
        /// drawn for one.
        origin: Option<PackageOrigin>,
        /// What the lockfile claims the package hashes to, so a later read of the registry
        /// can say whether the publisher agrees.
        integrity: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

/// The registry, not the package manager: pnpm, bun and yarn all resolve from npm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Cargo,
    Npm,
    Nix,
}

/// Where a locked package resolved from. A link is only honest when this says the package
/// came from the ecosystem's own registry, so the answer is kept whole rather than reduced
/// to a boolean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PackageOrigin {
    PublicRegistry,
    Registry { url: String },
    Remote { url: String },
    Local,
}

impl PackageOrigin {
    pub fn reference(&self) -> Option<&str> {
        match self {
            Self::Registry { url } | Self::Remote { url } => Some(url),
            Self::PublicRegistry | Self::Local => None,
        }
    }

    fn decode(kind: &str, reference: Option<String>) -> Result<Self, DatabaseError> {
        let unreadable = || DatabaseError::Unreadable {
            field: "origin_kind",
            value: kind.to_owned(),
        };

        match kind {
            "public_registry" => Ok(Self::PublicRegistry),
            "local" => Ok(Self::Local),
            "registry" => Ok(Self::Registry {
                url: reference.ok_or_else(unreadable)?,
            }),
            "remote" => Ok(Self::Remote {
                url: reference.ok_or_else(unreadable)?,
            }),
            _ => Err(unreadable()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    Introduced,
    Preexisting,
    Unknown,
}

/// Which way a dependency moved. A direction is only claimed when the two versions are
/// ordered, so a flake.lock pinning one commit sha over another reports `Changed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionMovement {
    Added,
    Removed,
    Upgraded,
    Downgraded,
    Changed,
}

impl Finding {
    pub async fn for_run(
        database: &Database,
        run_id: Id<Run>,
    ) -> Result<Vec<Finding>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT f.id, f.issue_id, f.location_kind, f.file_path, f.line_start, f.line_end, f.movement, \
                    f.ecosystem, f.package_name, f.package_version, f.origin_kind, f.origin_ref, \
                    f.package_integrity, f.severity, f.confidence, \
                    f.attribution, f.title, f.detail, \
                    i.fingerprint, i.fingerprint_version, i.fingerprint_canonical \
             FROM findings f JOIN issues i ON i.id = f.issue_id \
             WHERE f.run_id = ? ORDER BY f.id",
        )
        .bind(run_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.iter()
            .map(|row| Self::decode_row(row, run_id))
            .collect()
    }

    pub fn decode_row(row: &SqliteRow, run_id: Id<Run>) -> Result<Finding, DatabaseError> {
        Ok(Finding {
            movement: row
                .try_get::<Option<String>, _>("movement")?
                .map(|value| VersionMovement::from_stored(&value))
                .transpose()?,
            id: Id::from_raw(row.try_get("id")?),
            run_id,
            issue_id: Id::from_raw(row.try_get("issue_id")?),
            fingerprint: Fingerprint {
                version: row.try_get::<i64, _>("fingerprint_version")? as u32,
                hash: row.try_get("fingerprint")?,
                canonical: row.try_get("fingerprint_canonical")?,
            },
            location: Location::decode_row(row)?,
            severity: Severity::read(row, "severity")?,
            confidence: confidence_from_stored(row.try_get("confidence")?)?,
            attribution: Attribution::read(row, "attribution")?,
            title: row.try_get("title")?,
            detail: row.try_get("detail")?,
        })
    }
}

pub struct EncodedLocation {
    pub kind: &'static str,
    pub file_path: Option<String>,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub ecosystem: Option<&'static str>,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub origin_kind: Option<&'static str>,
    pub origin_ref: Option<String>,
    pub package_integrity: Option<String>,
}

impl Location {
    pub fn encoded(&self) -> EncodedLocation {
        match self {
            Location::File { path, span } => EncodedLocation {
                kind: "file",
                file_path: Some(path.as_str().to_owned()),
                line_start: span.map(|span| i64::from(span.start)),
                line_end: span.map(|span| i64::from(span.end)),
                ecosystem: None,
                package_name: None,
                package_version: None,
                origin_kind: None,
                origin_ref: None,
                package_integrity: None,
            },
            Location::Package {
                path,
                ecosystem,
                name,
                version,
                origin,
                integrity,
            } => EncodedLocation {
                kind: "package",
                file_path: Some(path.as_str().to_owned()),
                line_start: None,
                line_end: None,
                ecosystem: Some(ecosystem.stored()),
                package_name: Some(name.clone()),
                package_version: Some(version.clone()),
                origin_kind: origin.as_ref().map(StoredAs::stored),
                origin_ref: origin
                    .as_ref()
                    .and_then(PackageOrigin::reference)
                    .map(str::to_owned),
                package_integrity: integrity.clone(),
            },
        }
    }
}

impl DecodeRow for Location {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let kind: String = row.try_get("location_kind")?;
        match kind.as_str() {
            "file" => {
                let start: Option<i64> = row.try_get("line_start")?;
                let end: Option<i64> = row.try_get("line_end")?;
                let path: String = row.try_get("file_path")?;
                Ok(Location::File {
                    path: RepoPath::from_stored(&path, "file_path")?,
                    span: start.zip(end).map(|(start, end)| LineSpan {
                        start: start as u32,
                        end: end as u32,
                    }),
                })
            }
            "package" => {
                let path: String = row.try_get("file_path")?;
                let origin_kind: Option<String> = row.try_get("origin_kind")?;
                Ok(Location::Package {
                    path: RepoPath::from_stored(&path, "file_path")?,
                    ecosystem: Ecosystem::read(row, "ecosystem")?,
                    name: row.try_get("package_name")?,
                    version: row.try_get("package_version")?,
                    origin: origin_kind
                        .map(|kind| PackageOrigin::decode(&kind, row.try_get("origin_ref")?))
                        .transpose()?,
                    integrity: row.try_get("package_integrity")?,
                })
            }
            other => Err(DatabaseError::Unreadable {
                field: "location_kind",
                value: other.to_owned(),
            }),
        }
    }
}

impl StoredAs for PackageOrigin {
    fn stored(&self) -> &'static str {
        match self {
            PackageOrigin::PublicRegistry => "public_registry",
            PackageOrigin::Registry { .. } => "registry",
            PackageOrigin::Remote { .. } => "remote",
            PackageOrigin::Local => "local",
        }
    }
}

impl StoredAs for Severity {
    fn stored(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

impl FromStored for Severity {
    const FIELD: &'static str = "severity";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "info" => Some(Severity::Info),
            "low" => Some(Severity::Low),
            "medium" => Some(Severity::Medium),
            "high" => Some(Severity::High),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }
}

impl StoredAs for Attribution {
    fn stored(&self) -> &'static str {
        match self {
            Attribution::Introduced => "introduced",
            Attribution::Preexisting => "preexisting",
            Attribution::Unknown => "unknown",
        }
    }
}

impl FromStored for Attribution {
    const FIELD: &'static str = "attribution";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "introduced" => Some(Attribution::Introduced),
            "preexisting" => Some(Attribution::Preexisting),
            "unknown" => Some(Attribution::Unknown),
            _ => None,
        }
    }
}

impl StoredAs for Ecosystem {
    fn stored(&self) -> &'static str {
        match self {
            Ecosystem::Cargo => "cargo",
            Ecosystem::Npm => "npm",
            Ecosystem::Nix => "nix",
        }
    }
}

impl FromStored for Ecosystem {
    const FIELD: &'static str = "ecosystem";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "cargo" => Some(Ecosystem::Cargo),
            "npm" => Some(Ecosystem::Npm),
            "nix" => Some(Ecosystem::Nix),
            _ => None,
        }
    }
}

impl StoredAs for VersionMovement {
    fn stored(&self) -> &'static str {
        match self {
            VersionMovement::Added => "added",
            VersionMovement::Removed => "removed",
            VersionMovement::Upgraded => "upgraded",
            VersionMovement::Downgraded => "downgraded",
            VersionMovement::Changed => "changed",
        }
    }
}

impl FromStored for VersionMovement {
    const FIELD: &'static str = "movement";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "added" => Some(VersionMovement::Added),
            "removed" => Some(VersionMovement::Removed),
            "upgraded" => Some(VersionMovement::Upgraded),
            "downgraded" => Some(VersionMovement::Downgraded),
            "changed" => Some(VersionMovement::Changed),
            _ => None,
        }
    }
}
