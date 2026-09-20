use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::analysis::finding::fingerprint::Fingerprint;
use crate::prelude::*;

/// The persistent identity behind many findings. It exists so a human decision has
/// somewhere to live that survives the next scan.
///
/// It carries no open or resolved state. Whether an issue is open is a question about
/// one subject, and the same fingerprint can be open on one branch while resolved on
/// another. Lifecycle comes from comparing two runs; see [`crate::analysis::finding::compare`].
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

impl Issue {
    pub async fn load(
        database: &Database,
        issue_id: Id<Issue>,
    ) -> Result<Option<Issue>, DatabaseError> {
        let row = sqlx::query(
            "SELECT id, project_id, analyzer, fingerprint, fingerprint_version, \
                    fingerprint_canonical, triage, triage_reason, first_seen, last_seen \
             FROM issues WHERE id = ?",
        )
        .bind(issue_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(Issue {
            id: Id::from_raw(row.try_get("id")?),
            project_id: Id::from_raw(row.try_get("project_id")?),
            analyzer: row.try_get("analyzer")?,
            fingerprint: Fingerprint {
                version: row.try_get::<i64, _>("fingerprint_version")? as u32,
                hash: row.try_get("fingerprint")?,
                canonical: row.try_get("fingerprint_canonical")?,
            },
            triage: Triage::from_stored(row.try_get("triage")?, row.try_get("triage_reason")?)?,
            first_seen: Id::from_raw(row.try_get("first_seen")?),
            last_seen: Id::from_raw(row.try_get("last_seen")?),
        }))
    }
}

impl Triage {
    pub fn stored(&self) -> (&'static str, Option<String>) {
        match self {
            Triage::Untriaged => ("untriaged", None),
            Triage::Acknowledged => ("acknowledged", None),
            Triage::Muted { reason } => ("muted", Some(reason.clone())),
        }
    }

    pub fn from_stored(triage: String, reason: Option<String>) -> Result<Self, DatabaseError> {
        match (triage.as_str(), reason) {
            ("untriaged", _) => Ok(Triage::Untriaged),
            ("acknowledged", _) => Ok(Triage::Acknowledged),
            ("muted", Some(reason)) => Ok(Triage::Muted { reason }),
            _ => Err(DatabaseError::Unreadable {
                field: "triage",
                value: triage,
            }),
        }
    }
}
