pub mod compare;
pub mod fingerprint;
pub mod issue;

use serde::{Deserialize, Serialize};

use crate::analysis::Run;
use crate::confidence::Confidence;
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

/// A finding as an analyzer produces it, before the store resolves its issue and gives it an
/// identity.
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
