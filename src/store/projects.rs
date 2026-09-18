use std::str::FromStr;

use sqlx::Row;

use crate::analysis::runner;
use crate::forge::ForgeKind;
use crate::id::Id;
use crate::user::{ProjectMember, ProjectRole, User, UserRole};
use crate::vcs::{RemoteUrl, RepoPath};
use crate::watch::{Project, ProjectIcon};

use super::{Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMemberChange {
    Updated,
    Removed,
    Missing,
    FinalOwner,
}

impl Store {
    pub async fn set_project_member(
        &self,
        project_id: Id<Project>,
        user_id: Id<User>,
        role: ProjectRole,
    ) -> Result<ProjectMemberChange, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let existing_role: Option<String> =
            sqlx::query("SELECT role FROM project_members WHERE project_id = ? AND user_id = ?")
                .bind(project_id.raw())
                .bind(user_id.raw())
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| row.try_get("role"))
                .transpose()?;
        if existing_role.as_deref() == Some(ProjectRole::Owner.as_str())
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
        .bind(role.as_str())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(ProjectMemberChange::Updated)
    }

    pub async fn list_project_members(
        &self,
        project_id: Id<Project>,
    ) -> Result<Vec<ProjectMember>, StoreError> {
        let rows = sqlx::query(
            "SELECT m.user_id, u.display_name, m.role FROM project_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.project_id = ? ORDER BY u.display_name ASC",
        )
        .bind(project_id.raw())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(decode_project_member).collect()
    }

    pub async fn project_member(
        &self,
        project_id: Id<Project>,
        user_id: Id<User>,
    ) -> Result<Option<ProjectMember>, StoreError> {
        let row = sqlx::query(
            "SELECT m.user_id, u.display_name, m.role FROM project_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.project_id = ? AND m.user_id = ?",
        )
        .bind(project_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_project_member).transpose()
    }

    pub async fn remove_project_member(
        &self,
        project_id: Id<Project>,
        user_id: Id<User>,
    ) -> Result<ProjectMemberChange, StoreError> {
        let mut transaction = self.pool.begin().await?;
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
        if role == ProjectRole::Owner.as_str() {
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

    pub async fn count_project_owners(&self, project_id: Id<Project>) -> Result<i64, StoreError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS owners FROM project_members WHERE project_id = ? AND role = 'owner'",
        )
        .bind(project_id.raw())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("owners")?)
    }

    pub async fn create_project(
        &self,
        owner_id: Id<User>,
        name: &str,
        remote: RemoteUrl,
        forge_kind: ForgeKind,
        uses_default_analyzers: bool,
        analyzers: &[&str],
    ) -> Result<Project, StoreError> {
        validate_analyzers(analyzers)?;

        let id: Id<Project> = self.ids.next();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO projects (id, name, remote_url, forge_kind, uses_default_analyzers) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(name)
        .bind(remote.as_str())
        .bind(encode_forge_kind(forge_kind))
        .bind(i64::from(uses_default_analyzers))
        .execute(&mut *transaction)
        .await?;
        for analyzer in analyzers {
            sqlx::query("INSERT INTO project_analyzers (project_id, analyzer) VALUES (?, ?)")
                .bind(id.raw())
                .bind(analyzer)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role) VALUES (?, ?, 'owner')",
        )
        .bind(id.raw())
        .bind(owner_id.raw())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(Project {
            id,
            name: name.to_owned(),
            remote,
            forge_kind,
            description: None,
            icon: ProjectIcon::default(),
            uses_default_analyzers,
        })
    }

    pub async fn describe_project(
        &self,
        project_id: Id<Project>,
        description: Option<&str>,
        icon: &ProjectIcon,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE projects SET description = ?, icon_light_path = ?, icon_dark_path = ? \
             WHERE id = ?",
        )
        .bind(description)
        .bind(icon.light.as_ref().map(RepoPath::as_str))
        .bind(icon.dark.as_ref().map(RepoPath::as_str))
        .bind(project_id.raw())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, remote_url, forge_kind, description, icon_light_path, icon_dark_path, \
             uses_default_analyzers FROM projects ORDER BY id DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(decode_project).collect()
    }

    pub async fn list_projects_for(&self, user: &User) -> Result<Vec<Project>, StoreError> {
        match user.role {
            UserRole::Admin => return self.list_projects().await,
            UserRole::Guest => return Ok(Vec::new()),
            UserRole::Member => {}
        }
        let rows = sqlx::query(
            "SELECT p.id, p.name, p.remote_url, p.forge_kind, p.description, p.icon_light_path, p.icon_dark_path, \
             p.uses_default_analyzers FROM projects p \
             JOIN project_members m ON m.project_id = p.id \
             WHERE m.user_id = ? ORDER BY p.id DESC",
        )
        .bind(user.id.raw())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(decode_project).collect()
    }

    pub async fn project_role_for(
        &self,
        user: &User,
        project_id: Id<Project>,
    ) -> Result<Option<ProjectRole>, StoreError> {
        match user.role {
            UserRole::Admin => Ok(Some(ProjectRole::Owner)),
            UserRole::Guest => Ok(None),
            UserRole::Member => Ok(self
                .project_member(project_id, user.id)
                .await?
                .map(|member| member.role)),
        }
    }

    pub async fn project(&self, project_id: Id<Project>) -> Result<Option<Project>, StoreError> {
        let row = sqlx::query(
            "SELECT id, name, remote_url, forge_kind, description, icon_light_path, icon_dark_path, \
             uses_default_analyzers FROM projects WHERE id = ?",
        )
        .bind(project_id.raw())
        .fetch_optional(&self.pool)
        .await?;

        row.map(decode_project).transpose()
    }

    pub async fn set_project_analyzers(
        &self,
        project_id: Id<Project>,
        uses_default_analyzers: bool,
        analyzers: &[&str],
    ) -> Result<(), StoreError> {
        validate_analyzers(analyzers)?;

        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE projects SET uses_default_analyzers = ? WHERE id = ?")
            .bind(i64::from(uses_default_analyzers))
            .bind(project_id.raw())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM project_analyzers WHERE project_id = ?")
            .bind(project_id.raw())
            .execute(&mut *transaction)
            .await?;
        for analyzer in analyzers {
            sqlx::query("INSERT INTO project_analyzers (project_id, analyzer) VALUES (?, ?)")
                .bind(project_id.raw())
                .bind(analyzer)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    pub async fn custom_project_analyzers(
        &self,
        project_id: Id<Project>,
    ) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query(
            "SELECT analyzer FROM project_analyzers WHERE project_id = ? ORDER BY analyzer",
        )
        .bind(project_id.raw())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| row.try_get("analyzer").map_err(StoreError::from))
            .collect()
    }

    pub async fn effective_project_analyzers(
        &self,
        project: &Project,
    ) -> Result<Vec<String>, StoreError> {
        let custom_analyzers = if project.uses_default_analyzers {
            Vec::new()
        } else {
            self.custom_project_analyzers(project.id).await?
        };

        Ok(runner::effective_analyzers(
            project.uses_default_analyzers,
            custom_analyzers,
        ))
    }
}

fn decode_icon_path(path: Option<String>) -> Result<Option<RepoPath>, StoreError> {
    path.map(|path| {
        RepoPath::new(&path).map_err(|_| StoreError::Unreadable {
            field: "icon_path",
            value: path,
        })
    })
    .transpose()
}

fn validate_analyzers(analyzers: &[&str]) -> Result<(), StoreError> {
    analyzers
        .iter()
        .find(|analyzer| !runner::is_known_analyzer(analyzer))
        .map_or(Ok(()), |analyzer| {
            Err(StoreError::UnknownAnalyzer((*analyzer).to_owned()))
        })
}

fn decode_project(row: sqlx::sqlite::SqliteRow) -> Result<Project, StoreError> {
    let remote: String = row.try_get("remote_url")?;
    let remote = RemoteUrl::new(&remote).map_err(|_| StoreError::Unreadable {
        field: "remote_url",
        value: remote,
    })?;

    Ok(Project {
        id: Id::from_raw(row.try_get("id")?),
        name: row.try_get("name")?,
        remote,
        forge_kind: decode_forge_kind(row.try_get("forge_kind")?)?,
        description: row.try_get("description")?,
        icon: ProjectIcon {
            light: decode_icon_path(row.try_get("icon_light_path")?)?,
            dark: decode_icon_path(row.try_get("icon_dark_path")?)?,
        },
        uses_default_analyzers: match row.try_get("uses_default_analyzers")? {
            0 => false,
            1 => true,
            value => {
                return Err(StoreError::Unreadable {
                    field: "uses_default_analyzers",
                    value: value.to_string(),
                });
            }
        },
    })
}

fn decode_project_member(row: sqlx::sqlite::SqliteRow) -> Result<ProjectMember, StoreError> {
    let role: String = row.try_get("role")?;
    let role = ProjectRole::from_str(&role).ok_or(StoreError::Unreadable {
        field: "project_member.role",
        value: role,
    })?;
    Ok(ProjectMember {
        user_id: Id::from_raw(row.try_get("user_id")?),
        display_name: row.try_get("display_name")?,
        role,
    })
}

fn encode_forge_kind(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::Auto => "auto",
        ForgeKind::Github => "github",
        ForgeKind::Gitlab => "gitlab",
        ForgeKind::Gitea => "gitea",
        ForgeKind::Forgejo => "forgejo",
    }
}

fn decode_forge_kind(kind: String) -> Result<ForgeKind, StoreError> {
    match kind.as_str() {
        "auto" => Ok(ForgeKind::Auto),
        "github" => Ok(ForgeKind::Github),
        "gitlab" => Ok(ForgeKind::Gitlab),
        "gitea" => Ok(ForgeKind::Gitea),
        "forgejo" => Ok(ForgeKind::Forgejo),
        _ => Err(StoreError::Unreadable {
            field: "forge_kind",
            value: kind,
        }),
    }
}
