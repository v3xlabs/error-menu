use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::forge::{ForgeKind, ForgeMetadata};
use crate::id::Id;
use crate::vcs::{CommitSha, RemoteUrl, RepoPath};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id<Project>,
    pub name: String,
    pub remote: RemoteUrl,
    pub forge_kind: ForgeKind,
    pub description: Option<String>,
    pub icon: ProjectIcon,
    pub uses_default_analyzers: bool,
}

/// What we analyse. A subject outlives every snapshot of it: one pull request is one
/// subject through all of its pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub id: Id<Subject>,
    pub project_id: Id<Project>,
    pub kind: SubjectKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SubjectKind {
    Change { number: u64 },
    Branch { name: String },
    Commit { sha: CommitSha },
}

/// An immutable reading of a subject at one moment. A new head sha makes a new snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: Id<Snapshot>,
    pub subject_id: Id<Subject>,
    pub head: CommitSha,
    pub base: Option<CommitSha>,
    pub merge_base: Option<CommitSha>,
    pub forge: ForgeMetadata,
    pub observed_at: Timestamp,
}

/// Where a project's mark lives in its own repository. A path is kept rather than the
/// bytes, so the picture follows the default branch instead of going stale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIcon {
    pub light: Option<RepoPath>,
    pub dark: Option<RepoPath>,
}
