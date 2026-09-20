use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::analysis::Run;
use crate::analysis::confidence::Confidence;
use crate::analysis::confidence_from_stored;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::database::{Database, DatabaseError};
use crate::id::Id;
use crate::project::snapshot::Snapshot;

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

/// A signal as an analyzer produces it, before the database attaches it to a run.
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

impl Signal {
    pub async fn for_run(
        database: &Database,
        run_id: Id<Run>,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<Signal>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT signal_key, value_kind, score, flag, count_value, confidence, reason \
             FROM signals WHERE run_id = ? ORDER BY id",
        )
        .bind(run_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Signal {
                    run_id,
                    snapshot_id,
                    key: SignalKey::read(&row, "signal_key")?,
                    value: SignalValue::decode_row(&row)?,
                    confidence: confidence_from_stored(row.try_get("confidence")?)?,
                    reason: row.try_get("reason")?,
                })
            })
            .collect()
    }
}

impl StoredAs for SignalKey {
    fn stored(&self) -> &'static str {
        match self {
            SignalKey::OffTask => "off_task",
            SignalKey::DiffSize => "diff_size",
            SignalKey::BlastRadius => "blast_radius",
            SignalKey::DependencyRisk => "dependency_risk",
            SignalKey::TestsFailing => "tests_failing",
            SignalKey::LinksAdded => "links_added",
            SignalKey::RepositoryHygiene => "repository_hygiene",
        }
    }
}

impl FromStored for SignalKey {
    const FIELD: &'static str = "signal_key";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "off_task" => Some(SignalKey::OffTask),
            "diff_size" => Some(SignalKey::DiffSize),
            "blast_radius" => Some(SignalKey::BlastRadius),
            "dependency_risk" => Some(SignalKey::DependencyRisk),
            "tests_failing" => Some(SignalKey::TestsFailing),
            "links_added" => Some(SignalKey::LinksAdded),
            "repository_hygiene" => Some(SignalKey::RepositoryHygiene),
            _ => None,
        }
    }
}

impl DecodeRow for SignalValue {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let kind: String = row.try_get("value_kind")?;
        match kind.as_str() {
            "score" => {
                let Some(score) = row.try_get::<Option<f64>, _>("score")? else {
                    return Err(DatabaseError::Unreadable {
                        field: "signal_score",
                        value: "missing".to_owned(),
                    });
                };
                Score::new(score as f32)
                    .map(SignalValue::Score)
                    .ok_or(DatabaseError::Unreadable {
                        field: "signal_score",
                        value: score.to_string(),
                    })
            }
            "flag" => {
                let Some(flag) = row.try_get::<Option<bool>, _>("flag")? else {
                    return Err(DatabaseError::Unreadable {
                        field: "signal_flag",
                        value: "missing".to_owned(),
                    });
                };
                Ok(SignalValue::Flag(flag))
            }
            "count" => {
                let Some(count) = row.try_get::<Option<i64>, _>("count_value")? else {
                    return Err(DatabaseError::Unreadable {
                        field: "signal_count",
                        value: "missing".to_owned(),
                    });
                };
                u64::try_from(count).map(SignalValue::Count).map_err(|_| {
                    DatabaseError::Unreadable {
                        field: "signal_count",
                        value: count.to_string(),
                    }
                })
            }
            _ => Err(DatabaseError::Unreadable {
                field: "signal_value_kind",
                value: kind,
            }),
        }
    }
}
