pub mod icon;
pub mod member;
pub mod person;
pub mod snapshot;
pub mod subject;
pub mod transfer;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::analysis::runner;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::forge::ForgeKind;
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id<Project>,
    pub organization_id: Id<Organization>,
    pub name: String,
    pub remote: RemoteUrl,
    pub forge_kind: ForgeKind,
    pub description: Option<String>,
    pub icon: ProjectIcon,
    pub uses_default_analyzers: bool,
}

/// Where a project's mark lives in its own repository. A path is kept rather than the
/// bytes, so the picture follows the default branch instead of going stale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIcon {
    pub light: Option<RepoPath>,
    pub dark: Option<RepoPath>,
}

#[derive(Debug)]
pub struct ProjectSummary {
    pub project: Project,
    pub organization_name: String,
    pub viewer_role: ProjectRole,
    pub analyzers: Vec<String>,
    pub default_branch: Option<ProjectBranchSummary>,
}

#[derive(Debug)]
pub struct ProjectBranchSummary {
    pub snapshot_id: Id<Snapshot>,
    pub analyzers: Vec<ProjectAnalyzerSummary>,
}

#[derive(Debug)]
pub struct ProjectAnalyzerSummary {
    pub analyzer: String,
    pub status: String,
    pub finding_count: u64,
    pub tone: ProjectAnalyzerTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAnalyzerTone {
    Alarming,
    Attention,
    Clear,
    Running,
    Unscanned,
}

/// What the caller decides when a project is created. The creator becomes its owner, and
/// the analyzer list is ignored while `uses_default_analyzers` holds.
#[derive(Debug)]
pub struct NewProject<'a> {
    pub organization_id: Id<Organization>,
    pub owner_id: Id<User>,
    pub name: &'a str,
    pub remote: RemoteUrl,
    pub forge_kind: ForgeKind,
    pub uses_default_analyzers: bool,
    pub analyzers: &'a [&'a str],
}

impl Project {
    pub async fn create(
        database: &Database,
        new: NewProject<'_>,
    ) -> Result<Project, DatabaseError> {
        validate_analyzers(new.analyzers)?;

        let id: Id<Project> = database.ids.next();
        let mut transaction = database.write().await?;
        sqlx::query(
            "INSERT INTO projects (id, organization_id, name, remote_url, forge_kind, \
             uses_default_analyzers) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(new.organization_id.raw())
        .bind(new.name)
        .bind(new.remote.as_str())
        .bind(new.forge_kind.stored())
        .bind(i64::from(new.uses_default_analyzers))
        .execute(&mut *transaction)
        .await?;
        for analyzer in new.analyzers {
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
        .bind(new.owner_id.raw())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(Project {
            id,
            organization_id: new.organization_id,
            name: new.name.to_owned(),
            remote: new.remote,
            forge_kind: new.forge_kind,
            description: None,
            icon: ProjectIcon::default(),
            uses_default_analyzers: new.uses_default_analyzers,
        })
    }

    pub async fn load(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<Option<Project>, DatabaseError> {
        let row = sqlx::query(
            "SELECT id, organization_id, name, remote_url, forge_kind, description, \
             icon_light_path, icon_dark_path, uses_default_analyzers FROM projects WHERE id = ?",
        )
        .bind(project_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        row.as_ref().map(Project::decode_row).transpose()
    }

    pub async fn summaries_for(
        database: &Database,
        user: &User,
    ) -> Result<Vec<ProjectSummary>, DatabaseError> {
        let rows = sqlx::query(
            "WITH visible_projects AS MATERIALIZED ( \
                SELECT p.*, o.name AS organization_name, \
                    CASE WHEN ? THEN 'owner' \
                         WHEN m.role = 'owner' OR g.role = 'owner' THEN 'owner' \
                         WHEN m.role = 'operator' OR g.role = 'operator' THEN 'operator' \
                         ELSE 'viewer' END AS viewer_role, \
                    (SELECT json_group_array(analyzer) FROM ( \
                        SELECT analyzer FROM project_analyzers WHERE project_id = p.id \
                        ORDER BY analyzer \
                    )) AS custom_analyzers, \
                    (SELECT MAX((SELECT id FROM snapshots WHERE subject_id = s.id \
                                 ORDER BY id DESC LIMIT 1)) \
                     FROM subjects s WHERE s.project_id = p.id AND s.kind = 'branch') \
                    AS branch_snapshot_id \
                FROM projects p \
                JOIN organizations o ON o.id = p.organization_id \
                LEFT JOIN project_members m ON m.project_id = p.id AND m.user_id = ? \
                LEFT JOIN organization_members g \
                    ON g.organization_id = p.organization_id AND g.user_id = ? \
                WHERE ? OR m.user_id IS NOT NULL OR g.user_id IS NOT NULL \
             ) \
             SELECT p.*, r.id AS run_id, r.analyzer, r.status, \
                    COUNT(f.id) AS finding_count, \
                    COALESCE(MAX(CASE f.severity WHEN 'critical' THEN 2 WHEN 'high' THEN 2 \
                                               WHEN 'medium' THEN 1 ELSE 0 END), 0) \
                    AS severity_rank \
             FROM visible_projects p \
             LEFT JOIN snapshots n ON n.id = p.branch_snapshot_id \
             LEFT JOIN runs r ON r.id IN ( \
                 SELECT MAX(id) FROM runs \
                 WHERE snapshot_id = COALESCE(n.analysis_snapshot_id, n.id) GROUP BY analyzer \
             ) \
             LEFT JOIN findings f ON f.run_id = r.id \
             GROUP BY p.id, r.id ORDER BY p.id DESC, r.analyzer",
        )
        .bind(user.role == UserRole::Admin)
        .bind(user.id.raw())
        .bind(user.id.raw())
        .bind(user.role == UserRole::Admin)
        .fetch_all(&database.pool)
        .await?;

        let mut summaries: Vec<ProjectSummary> = Vec::new();
        for row in rows {
            let project_id = Id::from_raw(row.try_get("id")?);
            let analyzer = row
                .try_get::<Option<i64>, _>("run_id")?
                .is_some()
                .then(|| ProjectAnalyzerSummary::decode_row(&row))
                .transpose()?;
            if summaries
                .last()
                .is_none_or(|summary| summary.project.id != project_id)
            {
                let viewer_role = ProjectRole::read(&row, "viewer_role")?;
                let custom_analyzers: String = row.try_get("custom_analyzers")?;
                let custom_analyzers = serde_json::from_str(&custom_analyzers).map_err(|_| {
                    DatabaseError::Unreadable {
                        field: "project_analyzers",
                        value: custom_analyzers,
                    }
                })?;
                let default_branch =
                    row.try_get::<Option<i64>, _>("branch_snapshot_id")?
                        .map(|snapshot_id| ProjectBranchSummary {
                            snapshot_id: Id::from_raw(snapshot_id),
                            analyzers: Vec::new(),
                        });
                let project = Project::decode_row(&row)?;
                let analyzers =
                    runner::effective_analyzers(project.uses_default_analyzers, custom_analyzers);
                summaries.push(ProjectSummary {
                    project,
                    organization_name: row.try_get("organization_name")?,
                    viewer_role,
                    analyzers,
                    default_branch,
                });
            }
            if let Some(analyzer) = analyzer
                && let Some(branch) = summaries
                    .last_mut()
                    .and_then(|summary| summary.default_branch.as_mut())
            {
                branch.analyzers.push(analyzer);
            }
        }
        Ok(summaries)
    }

    /// Projects whose interval has elapsed. The comparison is done here rather than in SQL
    /// because a stored timestamp keeps nanoseconds and SQLite's date functions answer NULL
    /// for a format they do not recognise, which would stall the schedule in silence.
    pub async fn due(
        database: &Database,
        now: Timestamp,
    ) -> Result<Vec<Id<Project>>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, watch_interval_seconds, last_enqueued_at FROM projects ORDER BY id",
        )
        .fetch_all(&database.pool)
        .await?;
        let mut due = Vec::new();

        for row in rows {
            let interval: i64 = row.try_get("watch_interval_seconds")?;
            let elapsed = match row.try_get::<Option<String>, _>("last_enqueued_at")? {
                None => true,
                Some(value) => {
                    Timestamp::from_stored(&value)?
                        .duration_until(now)
                        .as_secs()
                        >= interval
                }
            };

            if elapsed {
                due.push(Id::from_raw(row.try_get("id")?));
            }
        }

        Ok(due)
    }

    pub async fn mark_enqueued(
        database: &Database,
        project_id: Id<Project>,
        at: Timestamp,
    ) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE projects SET last_enqueued_at = ? WHERE id = ?")
            .bind(at.to_string())
            .bind(project_id.raw())
            .execute(&database.pool)
            .await?;

        Ok(())
    }

    /// Moves the project, and records where it came from in the same transaction, so a
    /// reader can never find a project in an organization with nothing saying how it got
    /// there.
    pub async fn transfer(
        &self,
        database: &Database,
        from: &Organization,
        to: &Organization,
        moved_by: Id<User>,
    ) -> Result<(), DatabaseError> {
        let id: Id<ProjectTransfer> = database.ids.next();
        let moved_at = Timestamp::now();
        let mut transaction = database.write().await?;
        sqlx::query("UPDATE projects SET organization_id = ? WHERE id = ?")
            .bind(to.id.raw())
            .bind(self.id.raw())
            .execute(&mut *transaction)
            .await?;
        ProjectTransfer::record(&mut transaction, id, self.id, from, to, moved_by, moved_at)
            .await?;
        transaction.commit().await?;

        Ok(())
    }

    /// Deletes the project and everything keyed to it, deepest first.
    ///
    /// SQLite enforces no foreign key here: the pool sets `temp_store` and nothing else,
    /// so a missed table leaves rows pointing at an id that no longer exists rather than
    /// refusing the delete. The mirror on disk is deliberately left alone, because
    /// `Mirror::open` keys it by remote URL and a second project may watch the same
    /// remote.
    pub async fn delete(&self, database: &Database) -> Result<(), DatabaseError> {
        let project_id = self.id.raw();
        let mut transaction = database.write().await?;
        for statement in [
            "DELETE FROM findings WHERE run_id IN (SELECT r.id FROM runs r \
             JOIN snapshots n ON n.id = r.snapshot_id \
             JOIN subjects s ON s.id = n.subject_id WHERE s.project_id = ?)",
            "DELETE FROM signals WHERE run_id IN (SELECT r.id FROM runs r \
             JOIN snapshots n ON n.id = r.snapshot_id \
             JOIN subjects s ON s.id = n.subject_id WHERE s.project_id = ?)",
            "DELETE FROM runs WHERE id IN (SELECT r.id FROM runs r \
             JOIN snapshots n ON n.id = r.snapshot_id \
             JOIN subjects s ON s.id = n.subject_id WHERE s.project_id = ?)",
            "DELETE FROM check_runs WHERE snapshot_id IN (SELECT n.id FROM snapshots n \
             JOIN subjects s ON s.id = n.subject_id WHERE s.project_id = ?)",
            "DELETE FROM snapshot_people WHERE snapshot_id IN (SELECT n.id FROM snapshots n \
             JOIN subjects s ON s.id = n.subject_id WHERE s.project_id = ?)",
            "DELETE FROM commit_analyses WHERE project_id = ?",
            "DELETE FROM snapshots WHERE id IN (SELECT n.id FROM snapshots n \
             JOIN subjects s ON s.id = n.subject_id WHERE s.project_id = ?)",
            "DELETE FROM issues WHERE project_id = ?",
            "DELETE FROM subjects WHERE project_id = ?",
            "DELETE FROM jobs WHERE project_id = ?",
            "DELETE FROM project_analyzers WHERE project_id = ?",
            "DELETE FROM project_members WHERE project_id = ?",
            "DELETE FROM project_transfers WHERE project_id = ?",
            "DELETE FROM projects WHERE id = ?",
        ] {
            sqlx::query(statement)
                .bind(project_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    pub async fn custom_analyzers(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<Vec<String>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT analyzer FROM project_analyzers WHERE project_id = ? ORDER BY analyzer",
        )
        .bind(project_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.into_iter()
            .map(|row| row.try_get("analyzer").map_err(DatabaseError::from))
            .collect()
    }

    pub async fn describe(
        &self,
        database: &Database,
        description: Option<&str>,
        icon: &ProjectIcon,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE projects SET description = ?, icon_light_path = ?, icon_dark_path = ? \
             WHERE id = ?",
        )
        .bind(description)
        .bind(icon.light.as_ref().map(RepoPath::as_str))
        .bind(icon.dark.as_ref().map(RepoPath::as_str))
        .bind(self.id.raw())
        .execute(&database.pool)
        .await?;

        Ok(())
    }

    pub async fn set_analyzers(
        &self,
        database: &Database,
        uses_default_analyzers: bool,
        analyzers: &[&str],
    ) -> Result<(), DatabaseError> {
        validate_analyzers(analyzers)?;

        let mut transaction = database.write().await?;
        sqlx::query("UPDATE projects SET uses_default_analyzers = ? WHERE id = ?")
            .bind(i64::from(uses_default_analyzers))
            .bind(self.id.raw())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM project_analyzers WHERE project_id = ?")
            .bind(self.id.raw())
            .execute(&mut *transaction)
            .await?;
        for analyzer in analyzers {
            sqlx::query("INSERT INTO project_analyzers (project_id, analyzer) VALUES (?, ?)")
                .bind(self.id.raw())
                .bind(analyzer)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    pub async fn effective_analyzers(
        &self,
        database: &Database,
    ) -> Result<Vec<String>, DatabaseError> {
        let custom_analyzers = if self.uses_default_analyzers {
            Vec::new()
        } else {
            Project::custom_analyzers(database, self.id).await?
        };

        Ok(runner::effective_analyzers(
            self.uses_default_analyzers,
            custom_analyzers,
        ))
    }
}

impl DecodeRow for Project {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let remote: String = row.try_get("remote_url")?;
        let remote = RemoteUrl::new(&remote).map_err(|_| DatabaseError::Unreadable {
            field: "remote_url",
            value: remote,
        })?;

        Ok(Project {
            id: Id::from_raw(row.try_get("id")?),
            organization_id: Id::from_raw(row.try_get("organization_id")?),
            name: row.try_get("name")?,
            remote,
            forge_kind: ForgeKind::read(row, "forge_kind")?,
            description: row.try_get("description")?,
            icon: ProjectIcon {
                light: row
                    .try_get::<Option<&str>, _>("icon_light_path")?
                    .map(|path| RepoPath::from_stored(path, "icon_light_path"))
                    .transpose()?,
                dark: row
                    .try_get::<Option<&str>, _>("icon_dark_path")?
                    .map(|path| RepoPath::from_stored(path, "icon_dark_path"))
                    .transpose()?,
            },
            uses_default_analyzers: match row.try_get("uses_default_analyzers")? {
                0 => false,
                1 => true,
                value => {
                    return Err(DatabaseError::Unreadable {
                        field: "uses_default_analyzers",
                        value: value.to_string(),
                    });
                }
            },
        })
    }
}

impl DecodeRow for ProjectAnalyzerSummary {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let status: String = row.try_get("status")?;
        let tone = match status.as_str() {
            "failed" | "timed_out" => ProjectAnalyzerTone::Alarming,
            "running" => ProjectAnalyzerTone::Running,
            "skipped" => ProjectAnalyzerTone::Unscanned,
            "succeeded" => match row.try_get::<i64, _>("severity_rank")? {
                2 => ProjectAnalyzerTone::Alarming,
                1 => ProjectAnalyzerTone::Attention,
                _ => ProjectAnalyzerTone::Clear,
            },
            _ => {
                return Err(DatabaseError::Unreadable {
                    field: "run.status",
                    value: status,
                });
            }
        };
        let finding_count: i64 = row.try_get("finding_count")?;
        Ok(ProjectAnalyzerSummary {
            analyzer: row.try_get("analyzer")?,
            status,
            finding_count: finding_count
                .try_into()
                .map_err(|_| DatabaseError::Unreadable {
                    field: "finding_count",
                    value: finding_count.to_string(),
                })?,
            tone,
        })
    }
}

fn validate_analyzers(analyzers: &[&str]) -> Result<(), DatabaseError> {
    analyzers
        .iter()
        .find(|analyzer| !runner::is_known_analyzer(analyzer))
        .map_or(Ok(()), |analyzer| {
            Err(DatabaseError::UnknownAnalyzer((*analyzer).to_owned()))
        })
}
