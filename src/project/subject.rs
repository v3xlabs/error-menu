use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::DecodeRow;
use crate::database::{Database, DatabaseError};
use crate::id::Id;
use crate::project::Project;
use crate::vcs::CommitSha;

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

impl Subject {
    pub async fn upsert(
        database: &Database,
        project_id: Id<Project>,
        kind: SubjectKind,
    ) -> Result<Subject, DatabaseError> {
        let (kind_text, key) = kind.stored();
        let id: Id<Subject> = database.ids.next();

        let row = sqlx::query(
            "INSERT INTO subjects (id, project_id, kind, subject_key) VALUES (?, ?, ?, ?) \
             ON CONFLICT(project_id, kind, subject_key) DO NOTHING RETURNING id",
        )
        .bind(id.raw())
        .bind(project_id.raw())
        .bind(kind_text)
        .bind(&key)
        .fetch_optional(&database.pool)
        .await?;
        let id = match row {
            Some(row) => Id::from_raw(row.try_get("id")?),
            None => {
                let row = sqlx::query(
                    "SELECT id FROM subjects WHERE project_id = ? AND kind = ? AND subject_key = ?",
                )
                .bind(project_id.raw())
                .bind(kind_text)
                .bind(&key)
                .fetch_one(&database.pool)
                .await?;
                Id::from_raw(row.try_get("id")?)
            }
        };

        Ok(Subject {
            id,
            project_id,
            kind,
        })
    }
}

impl DecodeRow for Subject {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        Ok(Subject {
            id: Id::from_raw(row.try_get("subject_id")?),
            project_id: Id::from_raw(row.try_get("project_id")?),
            kind: SubjectKind::from_stored(
                row.try_get("subject_kind")?,
                row.try_get("subject_key")?,
            )?,
        })
    }
}

impl SubjectKind {
    fn stored(&self) -> (&'static str, String) {
        match self {
            SubjectKind::Change { number } => ("change", number.to_string()),
            SubjectKind::Branch { name } => ("branch", name.clone()),
            SubjectKind::Commit { sha } => ("commit", sha.to_string()),
        }
    }

    fn from_stored(kind: &str, key: &str) -> Result<Self, DatabaseError> {
        match kind {
            "change" => key
                .parse()
                .map(|number| SubjectKind::Change { number })
                .map_err(|_| DatabaseError::Unreadable {
                    field: "subject_key",
                    value: key.to_owned(),
                }),
            "branch" => Ok(SubjectKind::Branch {
                name: key.to_owned(),
            }),
            "commit" => {
                CommitSha::from_stored(key, "subject_key").map(|sha| SubjectKind::Commit { sha })
            }
            _ => Err(DatabaseError::Unreadable {
                field: "subject_kind",
                value: kind.to_owned(),
            }),
        }
    }
}
