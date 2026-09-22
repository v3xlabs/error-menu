use jiff::Timestamp;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::{DecodeRow, FromStored};
use crate::prelude::*;

/// One move of a project from one organization to another. The row is written in the same
/// transaction as the move, because the organization a project came from is overwritten by
/// it and no other row in this database holds it afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTransfer {
    pub id: Id<ProjectTransfer>,
    pub from_organization_name: String,
    pub to_organization_name: String,
    pub moved_by: Id<User>,
    pub moved_by_name: String,
    pub moved_at: Timestamp,
}

impl ProjectTransfer {
    /// Writes the record. Takes a connection rather than the pool because it belongs to the
    /// transaction that moves the project, never to one of its own.
    pub async fn record(
        transaction: &mut sqlx::SqliteConnection,
        id: Id<ProjectTransfer>,
        project_id: Id<Project>,
        from: &Organization,
        to: &Organization,
        moved_by: Id<User>,
        moved_at: Timestamp,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO project_transfers (id, project_id, from_organization_name, \
             to_organization_name, moved_by, moved_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(project_id.raw())
        .bind(from.name.as_str())
        .bind(to.name.as_str())
        .bind(moved_by.raw())
        .bind(moved_at.to_string())
        .execute(&mut *transaction)
        .await?;

        Ok(())
    }

    /// Newest first, because the question a reader asks is where the project came from
    /// last.
    pub async fn for_project(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<Vec<ProjectTransfer>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT t.id, t.from_organization_name, t.to_organization_name, \
                    t.moved_by, u.display_name AS moved_by_name, t.moved_at \
             FROM project_transfers t \
             JOIN users u ON u.id = t.moved_by \
             WHERE t.project_id = ? ORDER BY t.id DESC",
        )
        .bind(project_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(ProjectTransfer::decode_row).collect()
    }
}

impl DecodeRow for ProjectTransfer {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let moved_at = Timestamp::read(row, "moved_at")?;

        Ok(ProjectTransfer {
            id: Id::from_raw(row.try_get("id")?),
            from_organization_name: row.try_get("from_organization_name")?,
            to_organization_name: row.try_get("to_organization_name")?,
            moved_by: Id::from_raw(row.try_get("moved_by")?),
            moved_by_name: row.try_get("moved_by_name")?,
            moved_at,
        })
    }
}
