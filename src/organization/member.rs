use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::prelude::*;

/// Declaration order is privilege order: `Ord` is what resolves a caller who holds both an
/// organization grant and a project grant. Reordering these variants inverts that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrganizationRole {
    Viewer,
    Operator,
    Owner,
}

impl StoredAs for OrganizationRole {
    fn stored(&self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Owner => "owner",
        }
    }
}

impl FromStored for OrganizationRole {
    const FIELD: &'static str = "organization_member.role";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

impl OrganizationRole {
    pub async fn for_user(
        database: &Database,
        user: &User,
        organization_id: Id<Organization>,
    ) -> Result<Option<OrganizationRole>, DatabaseError> {
        if user.role == UserRole::Admin {
            return Ok(Some(OrganizationRole::Owner));
        }

        Ok(
            OrganizationMember::load(database, organization_id, user.id)
                .await?
                .map(|member| member.role),
        )
    }
}

impl From<OrganizationRole> for ProjectRole {
    fn from(role: OrganizationRole) -> Self {
        match role {
            OrganizationRole::Viewer => ProjectRole::Viewer,
            OrganizationRole::Operator => ProjectRole::Operator,
            OrganizationRole::Owner => ProjectRole::Owner,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationMember {
    pub user_id: Id<User>,
    pub display_name: String,
    pub role: OrganizationRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizationMemberChange {
    Updated,
    Removed,
    Missing,
    FinalOwner,
}

impl OrganizationMember {
    pub async fn set(
        database: &Database,
        organization_id: Id<Organization>,
        user_id: Id<User>,
        role: OrganizationRole,
    ) -> Result<OrganizationMemberChange, DatabaseError> {
        let mut transaction = database.write().await?;
        let existing_role: Option<String> = sqlx::query(
            "SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?",
        )
        .bind(organization_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| row.try_get("role"))
        .transpose()?;
        if existing_role.as_deref() == Some(OrganizationRole::Owner.stored())
            && role != OrganizationRole::Owner
            && sole_owner(&mut transaction, organization_id).await?
        {
            transaction.rollback().await?;
            return Ok(OrganizationMemberChange::FinalOwner);
        }
        sqlx::query(
            "INSERT INTO organization_members (organization_id, user_id, role) VALUES (?, ?, ?) \
             ON CONFLICT(organization_id, user_id) DO UPDATE SET role = excluded.role",
        )
        .bind(organization_id.raw())
        .bind(user_id.raw())
        .bind(role.stored())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(OrganizationMemberChange::Updated)
    }

    pub async fn list(
        database: &Database,
        organization_id: Id<Organization>,
    ) -> Result<Vec<OrganizationMember>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT m.user_id, u.display_name, m.role FROM organization_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.organization_id = ? ORDER BY u.display_name ASC",
        )
        .bind(organization_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(OrganizationMember::decode_row).collect()
    }

    pub async fn load(
        database: &Database,
        organization_id: Id<Organization>,
        user_id: Id<User>,
    ) -> Result<Option<OrganizationMember>, DatabaseError> {
        let row = sqlx::query(
            "SELECT m.user_id, u.display_name, m.role FROM organization_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.organization_id = ? AND m.user_id = ?",
        )
        .bind(organization_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        row.as_ref().map(OrganizationMember::decode_row).transpose()
    }

    pub async fn remove(
        database: &Database,
        organization_id: Id<Organization>,
        user_id: Id<User>,
    ) -> Result<OrganizationMemberChange, DatabaseError> {
        let mut transaction = database.write().await?;
        let role: Option<String> = sqlx::query(
            "SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?",
        )
        .bind(organization_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| row.try_get("role"))
        .transpose()?;
        let Some(role) = role else {
            transaction.rollback().await?;
            return Ok(OrganizationMemberChange::Missing);
        };
        if role == OrganizationRole::Owner.stored()
            && sole_owner(&mut transaction, organization_id).await?
        {
            transaction.rollback().await?;
            return Ok(OrganizationMemberChange::FinalOwner);
        }
        sqlx::query("DELETE FROM organization_members WHERE organization_id = ? AND user_id = ?")
            .bind(organization_id.raw())
            .bind(user_id.raw())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;

        Ok(OrganizationMemberChange::Removed)
    }

    /// The grant a user holds on the organization that owns one project. The resolver in
    /// `ProjectRole::for_user` reads this rather than loading the project first.
    pub async fn role_for_project(
        database: &Database,
        project_id: Id<Project>,
        user_id: Id<User>,
    ) -> Result<Option<OrganizationRole>, DatabaseError> {
        let row = sqlx::query(
            "SELECT m.role FROM organization_members m \
             JOIN projects p ON p.organization_id = m.organization_id \
             WHERE p.id = ? AND m.user_id = ?",
        )
        .bind(project_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        row.map(|row| OrganizationRole::read(&row, "role"))
            .transpose()
    }
}

async fn sole_owner(
    transaction: &mut sqlx::SqliteConnection,
    organization_id: Id<Organization>,
) -> Result<bool, DatabaseError> {
    let owners: i64 = sqlx::query(
        "SELECT COUNT(*) AS count FROM organization_members \
         WHERE organization_id = ? AND role = 'owner'",
    )
    .bind(organization_id.raw())
    .fetch_one(&mut *transaction)
    .await?
    .try_get("count")?;

    Ok(owners == 1)
}

impl DecodeRow for OrganizationMember {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        Ok(OrganizationMember {
            user_id: Id::from_raw(row.try_get("user_id")?),
            display_name: row.try_get("display_name")?,
            role: OrganizationRole::read(row, "role")?,
        })
    }
}
