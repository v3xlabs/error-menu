pub mod member;

use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::prelude::*;

/// A tenancy boundary. An organization owns projects and holds its own grants, so one
/// grant reaches every project inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub id: Id<Organization>,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug)]
pub struct OrganizationSummary {
    pub organization: Organization,
    pub viewer_role: OrganizationRole,
    pub project_count: u64,
}

/// What a deletion did. An organization that still owns projects is left alone: every
/// listing and every access check here reads `projects.organization_id`, so a project
/// without an organization is a project nobody can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizationDeletion {
    Deleted,
    HoldsProjects(u64),
}

impl Organization {
    pub async fn create(
        database: &Database,
        owner_id: Id<User>,
        name: &str,
        description: Option<&str>,
    ) -> Result<Organization, DatabaseError> {
        let id: Id<Organization> = database.ids.next();
        let mut transaction = database.write().await?;
        sqlx::query("INSERT INTO organizations (id, name, description) VALUES (?, ?, ?)")
            .bind(id.raw())
            .bind(name)
            .bind(description)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO organization_members (organization_id, user_id, role) VALUES (?, ?, ?)",
        )
        .bind(id.raw())
        .bind(owner_id.raw())
        .bind(OrganizationRole::Owner.stored())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(Organization {
            id,
            name: name.to_owned(),
            description: description.map(str::to_owned),
        })
    }

    pub async fn load(
        database: &Database,
        organization_id: Id<Organization>,
    ) -> Result<Option<Organization>, DatabaseError> {
        let row = sqlx::query("SELECT id, name, description FROM organizations WHERE id = ?")
            .bind(organization_id.raw())
            .fetch_optional(&database.pool)
            .await?;

        row.as_ref().map(Organization::decode_row).transpose()
    }

    pub async fn of_project(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<Option<Organization>, DatabaseError> {
        let row = sqlx::query(
            "SELECT o.id, o.name, o.description FROM organizations o \
             JOIN projects p ON p.organization_id = o.id WHERE p.id = ?",
        )
        .bind(project_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        row.as_ref().map(Organization::decode_row).transpose()
    }

    /// The organizations a caller can reach, with the grant they hold and how many
    /// projects sit inside. An administrator reaches every organization as an owner.
    pub async fn summaries_for(
        database: &Database,
        user: &User,
    ) -> Result<Vec<OrganizationSummary>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT o.id, o.name, o.description, \
                    CASE WHEN ? THEN 'owner' ELSE m.role END AS viewer_role, \
                    (SELECT COUNT(*) FROM projects p WHERE p.organization_id = o.id) \
                    AS project_count \
             FROM organizations o \
             LEFT JOIN organization_members m ON m.organization_id = o.id AND m.user_id = ? \
             WHERE ? OR m.user_id IS NOT NULL \
             ORDER BY o.id DESC",
        )
        .bind(user.role == UserRole::Admin)
        .bind(user.id.raw())
        .bind(user.role == UserRole::Admin)
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(OrganizationSummary::decode_row).collect()
    }

    pub async fn describe(
        &self,
        database: &Database,
        name: &str,
        description: Option<&str>,
    ) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE organizations SET name = ?, description = ? WHERE id = ?")
            .bind(name)
            .bind(description)
            .bind(self.id.raw())
            .execute(&database.pool)
            .await?;

        Ok(())
    }

    pub async fn delete(&self, database: &Database) -> Result<OrganizationDeletion, DatabaseError> {
        let mut transaction = database.write().await?;
        let projects: i64 =
            sqlx::query("SELECT COUNT(*) AS count FROM projects WHERE organization_id = ?")
                .bind(self.id.raw())
                .fetch_one(&mut *transaction)
                .await?
                .try_get("count")?;
        if projects > 0 {
            transaction.rollback().await?;
            let held = projects.try_into().map_err(|_| DatabaseError::Unreadable {
                field: "project_count",
                value: projects.to_string(),
            })?;

            return Ok(OrganizationDeletion::HoldsProjects(held));
        }
        sqlx::query("DELETE FROM organization_members WHERE organization_id = ?")
            .bind(self.id.raw())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id = ?")
            .bind(self.id.raw())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;

        Ok(OrganizationDeletion::Deleted)
    }
}

impl DecodeRow for Organization {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        Ok(Organization {
            id: Id::from_raw(row.try_get("id")?),
            name: row.try_get("name")?,
            description: row.try_get("description")?,
        })
    }
}

impl DecodeRow for OrganizationSummary {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let project_count: i64 = row.try_get("project_count")?;

        Ok(OrganizationSummary {
            organization: Organization::decode_row(row)?,
            viewer_role: OrganizationRole::read(row, "viewer_role")?,
            project_count: project_count
                .try_into()
                .map_err(|_| DatabaseError::Unreadable {
                    field: "project_count",
                    value: project_count.to_string(),
                })?,
        })
    }
}
