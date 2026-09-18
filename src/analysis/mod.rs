pub mod ci_checks;
pub mod hygiene;
pub mod links;
pub mod lockfile;
pub mod manifest;
pub mod repository_controls;
pub mod runner;
pub mod secret;
pub mod workflow;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::Id;
use crate::watch::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: Id<Run>,
    pub snapshot_id: Id<Snapshot>,
    pub analyzer: String,
    pub status: RunStatus,
    pub compared_against: Option<Id<Snapshot>>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// A run carries its own outcome so that "the analyzer failed" can never be read as
/// "the analyzer found nothing".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed { message: String },
    TimedOut,
    Skipped { reason: String },
}

impl RunStatus {
    pub fn is_comparable(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}
