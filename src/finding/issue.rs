use serde::{Deserialize, Serialize};

use crate::finding::fingerprint::Fingerprint;
use crate::id::Id;
use crate::watch::{Project, Snapshot};

/// The persistent identity behind many findings. It exists so a human decision has
/// somewhere to live that survives the next scan.
///
/// It carries no open or resolved state. Whether an issue is open is a question about
/// one subject, and the same fingerprint can be open on one branch while resolved on
/// another. Lifecycle comes from comparing two runs; see [`crate::finding::compare`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub id: Id<Issue>,
    pub project_id: Id<Project>,
    pub analyzer: String,
    pub fingerprint: Fingerprint,
    pub triage: Triage,
    pub first_seen: Id<Snapshot>,
    pub last_seen: Id<Snapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "triage")]
pub enum Triage {
    #[default]
    Untriaged,
    Acknowledged,
    Muted {
        reason: String,
    },
}
