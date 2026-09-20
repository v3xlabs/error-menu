use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRole {
    Viewer,
    Operator,
    Owner,
}

impl StoredAs for ProjectRole {
    fn stored(&self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Owner => "owner",
        }
    }
}

impl FromStored for ProjectRole {
    const FIELD: &'static str = "project_member.role";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

impl ProjectRole {
    pub const fn can_scan(self) -> bool {
        matches!(self, Self::Operator | Self::Owner)
    }

    pub const fn can_manage(self) -> bool {
        matches!(self, Self::Owner)
    }

    pub async fn for_user(
        database: &Database,
        user: &User,
        project_id: Id<Project>,
    ) -> Result<Option<ProjectRole>, DatabaseError> {
        match user.role {
            UserRole::Admin => Ok(Some(ProjectRole::Owner)),
            UserRole::Guest => Ok(None),
            UserRole::Member => Ok(ProjectMember::load(database, project_id, user.id)
                .await?
                .map(|member| member.role)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMember {
    pub user_id: Id<User>,
    pub display_name: String,
    pub role: ProjectRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMemberChange {
    Updated,
    Removed,
    Missing,
    FinalOwner,
}

impl ProjectMember {
    pub async fn set(
        database: &Database,
        project_id: Id<Project>,
        user_id: Id<User>,
        role: ProjectRole,
    ) -> Result<ProjectMemberChange, DatabaseError> {
        let mut transaction = database.pool.begin().await?;
        let existing_role: Option<String> =
            sqlx::query("SELECT role FROM project_members WHERE project_id = ? AND user_id = ?")
                .bind(project_id.raw())
                .bind(user_id.raw())
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| row.try_get("role"))
                .transpose()?;
        if existing_role.as_deref() == Some(ProjectRole::Owner.stored())
            && role != ProjectRole::Owner
        {
            let owners: i64 = sqlx::query(
                "SELECT COUNT(*) AS count FROM project_members WHERE project_id = ? AND role = 'owner'",
            )
            .bind(project_id.raw())
            .fetch_one(&mut *transaction)
            .await?
            .try_get("count")?;
            if owners == 1 {
                transaction.rollback().await?;
                return Ok(ProjectMemberChange::FinalOwner);
            }
        }
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role) VALUES (?, ?, ?) \
             ON CONFLICT(project_id, user_id) DO UPDATE SET role = excluded.role",
        )
        .bind(project_id.raw())
        .bind(user_id.raw())
        .bind(role.stored())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(ProjectMemberChange::Updated)
    }

    pub async fn list(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<Vec<ProjectMember>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT m.user_id, u.display_name, m.role FROM project_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.project_id = ? ORDER BY u.display_name ASC",
        )
        .bind(project_id.raw())
        .fetch_all(&database.pool)
        .await?;
        rows.iter().map(ProjectMember::decode_row).collect()
    }

    pub async fn load(
        database: &Database,
        project_id: Id<Project>,
        user_id: Id<User>,
    ) -> Result<Option<ProjectMember>, DatabaseError> {
        let row = sqlx::query(
            "SELECT m.user_id, u.display_name, m.role FROM project_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.project_id = ? AND m.user_id = ?",
        )
        .bind(project_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&database.pool)
        .await?;
        row.as_ref().map(ProjectMember::decode_row).transpose()
    }

    pub async fn remove(
        database: &Database,
        project_id: Id<Project>,
        user_id: Id<User>,
    ) -> Result<ProjectMemberChange, DatabaseError> {
        let mut transaction = database.pool.begin().await?;
        let role: Option<String> =
            sqlx::query("SELECT role FROM project_members WHERE project_id = ? AND user_id = ?")
                .bind(project_id.raw())
                .bind(user_id.raw())
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| row.try_get("role"))
                .transpose()?;
        let Some(role) = role else {
            transaction.rollback().await?;
            return Ok(ProjectMemberChange::Missing);
        };
        if role == ProjectRole::Owner.stored() {
            let owners: i64 = sqlx::query(
                "SELECT COUNT(*) AS count FROM project_members WHERE project_id = ? AND role = 'owner'",
            )
            .bind(project_id.raw())
            .fetch_one(&mut *transaction)
            .await?
            .try_get("count")?;
            if owners == 1 {
                transaction.rollback().await?;
                return Ok(ProjectMemberChange::FinalOwner);
            }
        }
        sqlx::query("DELETE FROM project_members WHERE project_id = ? AND user_id = ?")
            .bind(project_id.raw())
            .bind(user_id.raw())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(ProjectMemberChange::Removed)
    }
}

impl DecodeRow for ProjectMember {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let role = ProjectRole::read(row, "role")?;
        Ok(ProjectMember {
            user_id: Id::from_raw(row.try_get("user_id")?),
            display_name: row.try_get("display_name")?,
            role,
        })
    }
}
