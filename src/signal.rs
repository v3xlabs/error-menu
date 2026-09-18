use serde::{Deserialize, Serialize};

use crate::analysis::Run;
use crate::confidence::Confidence;
use crate::id::Id;
use crate::watch::Snapshot;

/// This run's opinion of the change as a whole. No location, no fingerprint, no
/// deduplication. Signals are the dashboard badges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub run_id: Id<Run>,
    pub snapshot_id: Id<Snapshot>,
    pub key: SignalKey,
    pub value: SignalValue,
    pub confidence: Confidence,
    pub reason: String,
}

/// A signal as an analyzer produces it, before the store attaches it to a run.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSignal {
    pub key: SignalKey,
    pub value: SignalValue,
    pub confidence: Confidence,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKey {
    OffTask,
    DiffSize,
    BlastRadius,
    DependencyRisk,
    TestsFailing,
    LinksAdded,
    RepositoryHygiene,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum SignalValue {
    Score(Score),
    Flag(bool),
    Count(u64),
}

/// Always measures concern, in one direction for every key: zero is calm, one is
/// alarming. That is why the key is `OffTask` and not `OnTask`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Score(f32);

impl Score {
    pub fn new(value: f32) -> Option<Self> {
        (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(Self(value))
    }

    pub fn value(self) -> f32 {
        self.0
    }
}
