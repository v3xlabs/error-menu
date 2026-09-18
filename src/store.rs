use std::str::FromStr;

use jiff::Timestamp;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::analysis::{Run, RunStatus, runner};
use crate::analysis::ci_checks::{CheckConclusion, CheckRun, CheckStatus};
use crate::confidence::Confidence;
use crate::finding::fingerprint::Fingerprint;
use crate::finding::issue::{Issue, Triage};
use crate::finding::{
    Attribution, Ecosystem, Finding, LineSpan, Location, NewFinding, Severity, VersionMovement,
};
use crate::forge::{ChangeState, ForgeKind, ForgeMetadata};
use crate::id::{Id, IdGenerator};
use crate::person::{Person, PersonRole, Signature};
use crate::queue::{self, Job, JobKind, JobRecord, JobState};
use crate::signal::{NewSignal, Score, Signal, SignalKey, SignalValue};
use crate::vcs::{CommitSha, RemoteUrl, RepoPath};
use crate::user::{ProjectMember, ProjectRole, User, UserRole};
use crate::watch::{Project, ProjectIcon, Snapshot, Subject, SubjectKind};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("stored {field} is not readable: {value:?}")]
    Unreadable { field: &'static str, value: String },
    #[error("analyzer is not implemented: {0}")]
    UnknownAnalyzer(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRoleChange {
    Updated,
    Missing,
    FinalAdmin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMemberChange {
    Updated,
    Removed,
    Missing,
    FinalOwner,
}

pub struct Store {
    pool: SqlitePool,
    ids: IdGenerator,
}

/// Everything one analyzer produced against one snapshot. Findings, signals and the run's
/// own status commit together, so a crashed worker leaves no half-recorded run.
pub struct RunRecord<'a> {
    pub snapshot_id: Id<Snapshot>,
    pub analyzer: &'a str,
    pub status: RunStatus,
    pub compared_against: Option<Id<Snapshot>>,
    pub findings: &'a [NewFinding],
    pub signals: &'a [NewSignal],
}

pub struct AnalysisRecord {
    pub subject: Subject,
    pub snapshot: Snapshot,
    pub check_runs: Vec<CheckRun>,
    pub runs: Vec<AnalysisRunRecord>,
}

pub struct AnalysisRunRecord {
    pub run: Run,
    pub findings: Vec<Finding>,
    pub signals: Vec<Signal>,
}

impl Store {
    /// One connection: SQLite takes one writer at a time, and the app is the only writer.
    pub async fn open(url: &str, node: u16) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            ids: IdGenerator::new(node),
        })
    }

    pub async fn register_user(
        &self,
        oidc_issuer: &str,
        oidc_subject: &str,
        display_name: &str,
        allow_registration: bool,
    ) -> Result<Option<User>, StoreError> {
        let now = Timestamp::now();
        let mut transaction = self.pool.begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users WHERE oidc_issuer = ? AND oidc_subject = ?",
        )
        .bind(oidc_issuer)
        .bind(oidc_subject)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let mut user = decode_user(row)?;
            sqlx::query("UPDATE users SET display_name = ?, last_signed_in_at = ? WHERE id = ?")
                .bind(display_name)
                .bind(now.to_string())
                .bind(user.id.raw())
                .execute(&mut *transaction)
                .await?;
            user.display_name = display_name.to_owned();
            user.last_signed_in_at = now;
            transaction.commit().await?;
            return Ok(Some(user));
        }

        if !allow_registration {
            transaction.rollback().await?;
            return Ok(None);
        }

        let user_count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM users")
            .fetch_one(&mut *transaction)
            .await?
            .try_get("count")?;
        let role = if user_count == 0 {
            UserRole::Admin
        } else {
            UserRole::Guest
        };
        let id: Id<User> = self.ids.next();
        sqlx::query(
            "INSERT INTO users (id, oidc_issuer, oidc_subject, display_name, role, access_role, created_at, last_signed_in_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(oidc_issuer)
        .bind(oidc_subject)
        .bind(display_name)
        .bind(role.legacy_storage_role())
        .bind(role.as_str())
        .bind(now.to_string())
        .bind(now.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(Some(User {
            id,
            oidc_issuer: oidc_issuer.to_owned(),
            oidc_subject: oidc_subject.to_owned(),
            display_name: display_name.to_owned(),
            role,
            created_at: now,
            last_signed_in_at: now,
        }))
    }

    pub async fn user(&self, user_id: Id<User>) -> Result<Option<User>, StoreError> {
        let row = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users WHERE id = ?",
        )
        .bind(user_id.raw())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_user).transpose()
    }

    pub async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, oidc_issuer, oidc_subject, display_name, access_role AS role, created_at, last_signed_in_at \
             FROM users ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(decode_user).collect()
    }

    pub async fn set_user_role(
        &self,
        user_id: Id<User>,
        role: UserRole,
    ) -> Result<UserRoleChange, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let current_role: Option<String> = sqlx::query("SELECT access_role FROM users WHERE id = ?")
            .bind(user_id.raw())
            .fetch_optional(&mut *transaction)
            .await?
            .map(|row| row.try_get("access_role"))
            .transpose()?;
        let Some(current_role) = current_role else {
            transaction.rollback().await?;
            return Ok(UserRoleChange::Missing);
        };
        let current_role = UserRole::from_str(&current_role).ok_or(StoreError::Unreadable {
            field: "user.access_role",
            value: current_role,
        })?;
        if current_role == UserRole::Admin && role != UserRole::Admin {
            let admins: i64 = sqlx::query("SELECT COUNT(*) AS count FROM users WHERE access_role = 'admin'")
                .fetch_one(&mut *transaction)
                .await?
                .try_get("count")?;
            if admins == 1 {
                transaction.rollback().await?;
                return Ok(UserRoleChange::FinalAdmin);
            }
        }
        sqlx::query("UPDATE users SET role = ?, access_role = ? WHERE id = ?")
            .bind(role.legacy_storage_role())
            .bind(role.as_str())
            .bind(user_id.raw())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(UserRoleChange::Updated)
    }

    async fn cleanup_expired_auth_records(&self, now_millis: i64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM auth_attempts WHERE expires_at_millis <= ?")
            .bind(now_millis)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE expires_at_millis <= ?")
            .bind(now_millis)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_auth_attempt(&self, state_hash: &str, expires_at: Timestamp) -> Result<(), StoreError> {
        self.cleanup_expired_auth_records(Timestamp::now().as_millisecond())
            .await?;
        sqlx::query("INSERT INTO auth_attempts (state_hash, expires_at_millis) VALUES (?, ?)")
            .bind(state_hash)
            .bind(expires_at.as_millisecond())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn consume_auth_attempt(&self, state_hash: &str, now: Timestamp) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let expires_at_millis: Option<i64> = sqlx::query("SELECT expires_at_millis FROM auth_attempts WHERE state_hash = ?")
            .bind(state_hash)
            .fetch_optional(&mut *transaction)
            .await?
            .map(|row| row.try_get("expires_at_millis"))
            .transpose()?;
        sqlx::query("DELETE FROM auth_attempts WHERE state_hash = ?")
            .bind(state_hash)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(expires_at_millis.is_some_and(|expires_at_millis| expires_at_millis > now.as_millisecond()))
    }

    pub async fn create_session(
        &self,
        user_id: Id<User>,
        token_hash: &str,
        expires_at: Timestamp,
    ) -> Result<(), StoreError> {
        self.cleanup_expired_auth_records(Timestamp::now().as_millisecond())
            .await?;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at_millis) VALUES (?, ?, ?)")
            .bind(token_hash)
            .bind(user_id.raw())
            .bind(expires_at.as_millisecond())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn user_for_session(&self, token_hash: &str, now: Timestamp) -> Result<Option<User>, StoreError> {
        self.cleanup_expired_auth_records(now.as_millisecond()).await?;
        let row = sqlx::query(
            "SELECT u.id, u.oidc_issuer, u.oidc_subject, u.display_name, u.access_role AS role, u.created_at, u.last_signed_in_at \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = ? AND s.expires_at_millis > ?",
        )
        .bind(token_hash)
        .bind(now.as_millisecond())
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_user).transpose()
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() != 0)
    }

    pub async fn set_project_member(
        &self,
        project_id: Id<Project>,
        user_id: Id<User>,
        role: ProjectRole,
    ) -> Result<ProjectMemberChange, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let existing_role: Option<String> = sqlx::query(
            "SELECT role FROM project_members WHERE project_id = ? AND user_id = ?",
        )
        .bind(project_id.raw())
        .bind(user_id.raw())
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| row.try_get("role"))
        .transpose()?;
        if existing_role.as_deref() == Some(ProjectRole::Owner.as_str()) && role != ProjectRole::Owner {
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
        let role: Option<String> = sqlx::query(
            "SELECT role FROM project_members WHERE project_id = ? AND user_id = ?",
        )
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
        sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES (?, ?, 'owner')")
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

    /// The newest head recorded for a branch subject of this project. Reading a file out
    /// of the repository needs a revision, and the default branch is the one a project's
    /// own mark should follow.
    pub async fn default_branch_head(
        &self,
        project_id: Id<Project>,
    ) -> Result<Option<CommitSha>, StoreError> {
        let row = sqlx::query(
            "SELECT n.head FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND s.kind = 'branch' ORDER BY n.id DESC LIMIT 1",
        )
        .bind(project_id.raw())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| decode_sha(row.try_get("head")?, "head"))
            .transpose()
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
            UserRole::Member => Ok(
                self.project_member(project_id, user.id)
                    .await?
                    .map(|member| member.role),
            ),
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

    pub async fn upsert_subject(
        &self,
        project_id: Id<Project>,
        kind: SubjectKind,
    ) -> Result<Subject, StoreError> {
        let (kind_text, key) = encode_subject_kind(&kind);
        let id: Id<Subject> = self.ids.next();

        sqlx::query(
            "INSERT OR IGNORE INTO subjects (id, project_id, kind, subject_key) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(project_id.raw())
        .bind(kind_text)
        .bind(&key)
        .execute(&self.pool)
        .await?;

        let row = sqlx::query(
            "SELECT id FROM subjects WHERE project_id = ? AND kind = ? AND subject_key = ?",
        )
        .bind(project_id.raw())
        .bind(kind_text)
        .bind(&key)
        .fetch_one(&self.pool)
        .await?;

        Ok(Subject {
            id: Id::from_raw(row.try_get("id")?),
            project_id,
            kind,
        })
    }

    pub async fn record_snapshot(
        &self,
        subject_id: Id<Subject>,
        head: CommitSha,
        base: Option<CommitSha>,
        merge_base: Option<CommitSha>,
        forge: ForgeMetadata,
    ) -> Result<Snapshot, StoreError> {
        let id: Id<Snapshot> = self.ids.next();
        let observed_at = Timestamp::now();
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO snapshots \
             (id, subject_id, head, base, merge_base, forge_title, forge_body, forge_author, forge_url, forge_base_ref, forge_head_ref, forge_state, forge_merge_commit, signature_present, signature_verified, signature_signer, signature_reason, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(subject_id.raw())
        .bind(head.as_str())
        .bind(base.as_ref().map(CommitSha::as_str))
        .bind(merge_base.as_ref().map(CommitSha::as_str))
        .bind(forge.title.as_deref())
        .bind(forge.body.as_deref())
        .bind(forge.author.as_deref())
        .bind(forge.url.as_deref())
        .bind(forge.base_ref.as_deref())
        .bind(forge.head_ref.as_deref())
        .bind(forge.state.map(encode_change_state))
        .bind(forge.merge_commit.as_ref().map(CommitSha::as_str))
        .bind(i64::from(forge.signature.present))
        .bind(forge.signature.verified.map(i64::from))
        .bind(forge.signature.signer.as_deref())
        .bind(forge.signature.reason.as_deref())
        .bind(observed_at.to_string())
        .execute(&mut *transaction)
        .await?;

        for person in &forge.people {
            sqlx::query(
                "INSERT INTO snapshot_people \
                 (id, snapshot_id, role, person_name, email, login, avatar_url, identity) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(self.ids.next::<Person>().raw())
            .bind(id.raw())
            .bind(encode_person_role(person.role))
            .bind(person.name.as_deref())
            .bind(person.email.as_deref())
            .bind(person.login.as_deref())
            .bind(person.avatar_url.as_deref())
            .bind(person.identity())
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        Ok(Snapshot {
            id,
            subject_id,
            head,
            base,
            merge_base,
            forge,
            observed_at,
        })
    }

    /// The newest reading of one change, with the people it named. Re-scanning a change
    /// that discovery only recorded needs the range and the metadata it was recorded with.
    pub async fn latest_snapshot_for_change(
        &self,
        project_id: Id<Project>,
        number: u64,
    ) -> Result<Option<Snapshot>, StoreError> {
        let row = sqlx::query(
            "SELECT n.id, n.subject_id, n.head, n.base, n.merge_base, n.forge_title, n.forge_body, \
                    n.forge_author, n.forge_url, n.forge_base_ref, n.forge_head_ref, n.forge_state, \
                    n.forge_merge_commit, n.signature_present, n.signature_verified, \
                    n.signature_signer, n.signature_reason, n.observed_at \
             FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND s.kind = 'change' AND s.subject_key = ? \
             ORDER BY n.id DESC LIMIT 1",
        )
        .bind(project_id.raw())
        .bind(number.to_string())
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let mut snapshot = decode_snapshot(row)?;
        snapshot.forge.people = self.people_for_snapshot(snapshot.id).await?;

        Ok(Some(snapshot))
    }
    pub async fn people_for_snapshot(
        &self,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<Person>, StoreError> {
        let rows = sqlx::query(
            "SELECT role, person_name, email, login, avatar_url \
             FROM snapshot_people WHERE snapshot_id = ? ORDER BY id",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Person {
                    role: decode_person_role(row.try_get("role")?)?,
                    name: row.try_get("person_name")?,
                    email: row.try_get("email")?,
                    login: row.try_get("login")?,
                    avatar_url: row.try_get("avatar_url")?,
                })
            })
            .collect()
    }

    /// The avatar a forge last gave for this person. One human reaches us under several
    /// identities: the forge knows them by login, while their commits are keyed by the
    /// address they committed with. When the address has no picture, the same login does,
    /// so a bot that opened a change is not drawn twice in two different ways.
    pub async fn avatar_url_for_identity(
        &self,
        identity: &str,
    ) -> Result<Option<String>, StoreError> {
        let own = sqlx::query(
            "SELECT avatar_url FROM snapshot_people \
             WHERE identity = ? AND avatar_url IS NOT NULL ORDER BY id DESC LIMIT 1",
        )
        .bind(identity)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = own {
            return Ok(Some(row.try_get("avatar_url")?));
        }

        let row = sqlx::query(
            "SELECT other.avatar_url FROM snapshot_people mine \
             JOIN snapshot_people other \
               ON other.login IS NOT NULL \
              AND other.avatar_url IS NOT NULL \
              AND lower(other.login) IN (lower(mine.login), lower(mine.person_name)) \
             WHERE mine.identity = ? ORDER BY other.id DESC LIMIT 1",
        )
        .bind(identity)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| row.try_get("avatar_url")).transpose()?)
    }

    pub async fn record_run(&self, record: RunRecord<'_>) -> Result<Run, StoreError> {
        let started_at = Timestamp::now();
        let finished_at = (!matches!(record.status, RunStatus::Running)).then(Timestamp::now);
        let (status_text, status_detail) = encode_run_status(&record.status);
        let id: Id<Run> = self.ids.next();

        let mut transaction = self.pool.begin().await?;
        let project_id = project_of_snapshot(&mut transaction, record.snapshot_id).await?;

        sqlx::query(
            "INSERT INTO runs \
             (id, snapshot_id, analyzer, status, status_detail, compared_against, started_at, finished_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(record.snapshot_id.raw())
        .bind(record.analyzer)
        .bind(status_text)
        .bind(status_detail)
        .bind(record.compared_against.map(Id::raw))
        .bind(started_at.to_string())
        .bind(finished_at.map(|at| at.to_string()))
        .execute(&mut *transaction)
        .await?;

        for finding in record.findings {
            let issue_id = resolve_issue(
                &mut transaction,
                &self.ids,
                project_id,
                record.analyzer,
                &finding.fingerprint,
                record.snapshot_id,
            )
            .await?;
            insert_finding(&mut transaction, &self.ids, id, issue_id, finding).await?;
        }

        for signal in record.signals {
            insert_signal(&mut transaction, &self.ids, id, signal).await?;
        }

        transaction.commit().await?;

        Ok(Run {
            id,
            snapshot_id: record.snapshot_id,
            analyzer: record.analyzer.to_owned(),
            status: record.status,
            compared_against: record.compared_against,
            started_at,
            finished_at,
        })
    }

    pub async fn findings_for_run(&self, run_id: Id<Run>) -> Result<Vec<Finding>, StoreError> {
        let rows = sqlx::query(
            "SELECT f.id, f.issue_id, f.location_kind, f.file_path, f.line_start, f.line_end, f.movement, \
                    f.ecosystem, f.package_name, f.package_version, f.severity, f.confidence, \
                    f.attribution, f.title, f.detail, \
                    i.fingerprint, i.fingerprint_version, i.fingerprint_canonical \
             FROM findings f JOIN issues i ON i.id = f.issue_id \
             WHERE f.run_id = ? ORDER BY f.id",
        )
        .bind(run_id.raw())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Finding {
                    movement: decode_movement(row.try_get("movement")?)?,
                    id: Id::from_raw(row.try_get("id")?),
                    run_id,
                    issue_id: Id::from_raw(row.try_get("issue_id")?),
                    fingerprint: Fingerprint {
                        version: row.try_get::<i64, _>("fingerprint_version")? as u32,
                        hash: row.try_get("fingerprint")?,
                        canonical: row.try_get("fingerprint_canonical")?,
                    },
                    location: decode_location(&row)?,
                    severity: decode_severity(row.try_get("severity")?)?,
                    confidence: decode_confidence(row.try_get("confidence")?)?,
                    attribution: decode_attribution(row.try_get("attribution")?)?,
                    title: row.try_get("title")?,
                    detail: row.try_get("detail")?,
                })
            })
            .collect()
    }

    pub async fn analyses_for_project(
        &self,
        project_id: Id<Project>,
    ) -> Result<Vec<AnalysisRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT s.id AS subject_id, s.project_id, s.kind AS subject_kind, s.subject_key, \
                    n.id, n.head, n.base, n.merge_base, n.forge_title, n.forge_body, n.forge_author, \
                    n.forge_url, n.forge_base_ref, n.forge_head_ref, n.forge_state, n.forge_merge_commit, \
                    n.signature_present, n.signature_verified, n.signature_signer, n.signature_reason, n.observed_at \
             FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? ORDER BY n.id DESC",
        )
        .bind(project_id.raw())
        .fetch_all(&self.pool)
        .await?;

        let mut analyses = Vec::with_capacity(rows.len());
        for row in rows {
            let subject = decode_subject(&row)?;
            let mut snapshot = decode_snapshot(row)?;
            snapshot.forge.people = self.people_for_snapshot(snapshot.id).await?;
            let check_runs = self.check_runs_for_snapshot(snapshot.id).await?;
            let runs = self.runs_for_snapshot(snapshot.id).await?;
            analyses.push(AnalysisRecord {
                subject,
                snapshot,
                check_runs,
                runs,
            });
        }

        Ok(analyses)
    }

    pub async fn replace_check_runs(
        &self,
        snapshot_id: Id<Snapshot>,
        check_runs: &[CheckRun],
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM check_runs WHERE snapshot_id = ?")
            .bind(snapshot_id.raw())
            .execute(&mut *transaction)
            .await?;
        for check_run in check_runs {
            sqlx::query(
                "INSERT INTO check_runs \
                 (snapshot_id, name, status, conclusion, url, log_excerpt_ref) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(snapshot_id.raw())
            .bind(&check_run.name)
            .bind(encode_check_status(check_run.status))
            .bind(check_run.conclusion.map(encode_check_conclusion))
            .bind(&check_run.url)
            .bind(&check_run.log_excerpt_ref)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    async fn check_runs_for_snapshot(
        &self,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<CheckRun>, StoreError> {
        let rows = sqlx::query(
            "SELECT name, status, conclusion, url, log_excerpt_ref \
             FROM check_runs WHERE snapshot_id = ? ORDER BY id",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(CheckRun {
                    name: row.try_get("name")?,
                    status: decode_check_status(row.try_get("status")?)?,
                    conclusion: row
                        .try_get::<Option<String>, _>("conclusion")?
                        .map(decode_check_conclusion)
                        .transpose()?,
                    url: row.try_get("url")?,
                    log_excerpt_ref: row.try_get("log_excerpt_ref")?,
                })
            })
            .collect()
    }
    pub async fn signals_for_run(
        &self,
        run_id: Id<Run>,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<Signal>, StoreError> {
        let rows = sqlx::query(
            "SELECT signal_key, value_kind, score, flag, count_value, confidence, reason \
             FROM signals WHERE run_id = ? ORDER BY id",
        )
        .bind(run_id.raw())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Signal {
                    run_id,
                    snapshot_id,
                    key: decode_signal_key(row.try_get("signal_key")?)?,
                    value: decode_signal_value(&row)?,
                    confidence: decode_confidence(row.try_get("confidence")?)?,
                    reason: row.try_get("reason")?,
                })
            })
            .collect()
    }

    async fn runs_for_snapshot(
        &self,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<AnalysisRunRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, analyzer, status, status_detail, compared_against, started_at, finished_at \
             FROM runs WHERE snapshot_id = ? ORDER BY id",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&self.pool)
        .await?;

        let mut runs = Vec::with_capacity(rows.len());
        for row in rows {
            let run = decode_run(row, snapshot_id)?;
            let findings = self.findings_for_run(run.id).await?;
            let signals = self.signals_for_run(run.id, snapshot_id).await?;
            runs.push(AnalysisRunRecord {
                run,
                findings,
                signals,
            });
        }

        Ok(runs)
    }

    /// The baseline for a comparison: the newest successful run of this analyzer on any
    /// earlier snapshot of the same subject. A failed run is not a baseline, because
    /// "found nothing" and "could not look" are different answers.
    pub async fn last_successful_analyzer(
        &self,
        subject_id: Id<Subject>,
        analyzer: &str,
        before: Id<Snapshot>,
    ) -> Result<Option<Run>, StoreError> {
        let row = sqlx::query(
            "SELECT r.id, r.snapshot_id, r.status_detail, r.compared_against, r.started_at, r.finished_at \
             FROM runs r JOIN snapshots s ON s.id = r.snapshot_id \
             WHERE s.subject_id = ? AND r.analyzer = ? AND r.status = 'succeeded' AND s.id < ? \
             ORDER BY r.id DESC LIMIT 1",
        )
        .bind(subject_id.raw())
        .bind(analyzer)
        .bind(before.raw())
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(Run {
            id: Id::from_raw(row.try_get("id")?),
            snapshot_id: Id::from_raw(row.try_get("snapshot_id")?),
            analyzer: analyzer.to_owned(),
            status: RunStatus::Succeeded,
            compared_against: row
                .try_get::<Option<i64>, _>("compared_against")?
                .map(Id::from_raw),
            started_at: decode_timestamp(row.try_get("started_at")?)?,
            finished_at: row
                .try_get::<Option<String>, _>("finished_at")?
                .map(decode_timestamp)
                .transpose()?,
        }))
    }

    pub async fn issue(&self, issue_id: Id<Issue>) -> Result<Option<Issue>, StoreError> {
        let row = sqlx::query(
            "SELECT id, project_id, analyzer, fingerprint, fingerprint_version, \
                    fingerprint_canonical, triage, triage_reason, first_seen, last_seen \
             FROM issues WHERE id = ?",
        )
        .bind(issue_id.raw())
        .fetch_optional(&self.pool)
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
            triage: decode_triage(row.try_get("triage")?, row.try_get("triage_reason")?)?,
            first_seen: Id::from_raw(row.try_get("first_seen")?),
            last_seen: Id::from_raw(row.try_get("last_seen")?),
        }))
    }

    /// Puts a project in the queue unless it is already there. A project that is waiting or
    /// running does not need a second job: the work reads the forge when it starts, so a
    /// queued job already covers anything that changed since it was queued.
    pub async fn enqueue(
        &self,
        project_id: Id<Project>,
        kind: JobKind,
    ) -> Result<Option<Id<Job>>, StoreError> {
        let id: Id<Job> = self.ids.next();
        let now = Timestamp::now().to_string();
        let inserted = sqlx::query(
            "INSERT INTO jobs (id, project_id, kind, state, available_at, created_at) \
             SELECT ?, ?, ?, 'queued', ?, ? \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM jobs \
                 WHERE project_id = ? AND kind = ? AND state IN ('queued', 'running') \
             )",
        )
        .bind(id.raw())
        .bind(project_id.raw())
        .bind(queue::encode_kind(kind))
        .bind(&now)
        .bind(&now)
        .bind(project_id.raw())
        .bind(queue::encode_kind(kind))
        .execute(&self.pool)
        .await?;

        Ok((inserted.rows_affected() > 0).then_some(id))
    }

    /// Takes the oldest ready job of the highest priority and holds it for the lease. The
    /// claim is one statement, so two workers cannot take the same job.
    pub async fn claim(
        &self,
        worker: &str,
        lease: std::time::Duration,
    ) -> Result<Option<Job>, StoreError> {
        let now = Timestamp::now();
        let expires = now + lease;
        let row = sqlx::query(
            "UPDATE jobs SET state = 'running', claimed_by = ?, lease_expires_at = ?, \
                    attempts = attempts + 1 \
             WHERE id = ( \
                 SELECT id FROM jobs WHERE state = 'queued' AND available_at <= ? \
                 ORDER BY priority DESC, id LIMIT 1 \
             ) \
             RETURNING id, project_id, kind, attempts",
        )
        .bind(worker)
        .bind(expires.to_string())
        .bind(now.to_string())
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let kind_text: String = row.try_get("kind")?;
        let kind = queue::decode_kind(&kind_text).ok_or(StoreError::Unreadable {
            field: "job_kind",
            value: kind_text,
        })?;

        Ok(Some(Job {
            id: Id::from_raw(row.try_get("id")?),
            project_id: Id::from_raw(row.try_get("project_id")?),
            kind,
            attempts: row.try_get("attempts")?,
        }))
    }

    pub async fn finish(&self, job_id: Id<Job>) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE jobs SET state = 'done', claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = NULL, finished_at = ? WHERE id = ?",
        )
        .bind(Timestamp::now().to_string())
        .bind(job_id.raw())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// A job with a retry time goes back in the queue and keeps its error for the record.
    /// Without one it is finished as failed, so the schedule can queue a fresh job later
    /// instead of retrying this one for ever.
    pub async fn fail(
        &self,
        job_id: Id<Job>,
        message: &str,
        retry_at: Option<Timestamp>,
    ) -> Result<(), StoreError> {
        let state = match retry_at {
            Some(_) => JobState::Queued,
            None => JobState::Failed,
        };

        sqlx::query(
            "UPDATE jobs SET state = ?, claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = ?, available_at = COALESCE(?, available_at), finished_at = ? \
             WHERE id = ?",
        )
        .bind(queue::encode_state(state))
        .bind(message)
        .bind(retry_at.map(|at| at.to_string()))
        .bind(retry_at.is_none().then(|| Timestamp::now().to_string()))
        .bind(job_id.raw())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// A worker that dies mid-job leaves its claim behind. The lease is what makes that
    /// recoverable without a human, so an expired one returns the job to the queue.
    pub async fn reclaim_expired_leases(&self) -> Result<u64, StoreError> {
        let affected = sqlx::query(
            "UPDATE jobs SET state = 'queued', claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = 'lease expired' \
             WHERE state = 'running' AND lease_expires_at IS NOT NULL AND lease_expires_at < ?",
        )
        .bind(Timestamp::now().to_string())
        .execute(&self.pool)
        .await?;

        Ok(affected.rows_affected())
    }

    /// Projects whose interval has elapsed. The comparison is done here rather than in SQL
    /// because a stored timestamp keeps nanoseconds and SQLite's date functions answer NULL
    /// for a format they do not recognise, which would stall the schedule in silence.
    pub async fn projects_due(&self, now: Timestamp) -> Result<Vec<Id<Project>>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_interval_seconds, last_enqueued_at FROM projects ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut due = Vec::new();

        for row in rows {
            let interval: i64 = row.try_get("watch_interval_seconds")?;
            let elapsed = match row.try_get::<Option<String>, _>("last_enqueued_at")? {
                None => true,
                Some(value) => {
                    decode_timestamp(value)?.duration_until(now).as_secs() >= interval
                }
            };

            if elapsed {
                due.push(Id::from_raw(row.try_get("id")?));
            }
        }

        Ok(due)
    }

    /// The newest jobs, so a reader can see that scanning happens without them. A project
    /// filter answers the question a project page asks.
    pub async fn recent_jobs(
        &self,
        project_id: Option<Id<Project>>,
        limit: i64,
    ) -> Result<Vec<JobRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, project_id, kind, state, attempts, last_error, available_at, \
                    created_at, finished_at \
             FROM jobs WHERE (?1 IS NULL OR project_id = ?1) ORDER BY id DESC LIMIT ?2",
        )
        .bind(project_id.map(Id::raw))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let kind_text: String = row.try_get("kind")?;
                let state_text: String = row.try_get("state")?;

                Ok(JobRecord {
                    id: Id::from_raw(row.try_get("id")?),
                    project_id: Id::from_raw(row.try_get("project_id")?),
                    kind: queue::decode_kind(&kind_text).ok_or(StoreError::Unreadable {
                        field: "job_kind",
                        value: kind_text,
                    })?,
                    state: queue::decode_state(&state_text).ok_or(StoreError::Unreadable {
                        field: "job_state",
                        value: state_text,
                    })?,
                    attempts: row.try_get("attempts")?,
                    last_error: row.try_get("last_error")?,
                    available_at: decode_timestamp(row.try_get("available_at")?)?,
                    created_at: decode_timestamp(row.try_get("created_at")?)?,
                    finished_at: row
                        .try_get::<Option<String>, _>("finished_at")?
                        .map(decode_timestamp)
                        .transpose()?,
                })
            })
            .collect()
    }

    pub async fn mark_enqueued(&self, project_id: Id<Project>, at: Timestamp) -> Result<(), StoreError> {
        sqlx::query("UPDATE projects SET last_enqueued_at = ? WHERE id = ?")
            .bind(at.to_string())
            .bind(project_id.raw())
            .execute(&self.pool)
            .await?;

        Ok(())
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

fn decode_user(row: sqlx::sqlite::SqliteRow) -> Result<User, StoreError> {
    let role: String = row.try_get("role")?;
    let role = UserRole::from_str(&role).ok_or(StoreError::Unreadable {
        field: "user.role",
        value: role,
    })?;

    Ok(User {
        id: Id::from_raw(row.try_get("id")?),
        oidc_issuer: row.try_get("oidc_issuer")?,
        oidc_subject: row.try_get("oidc_subject")?,
        display_name: row.try_get("display_name")?,
        role,
        created_at: decode_timestamp(row.try_get("created_at")?)?,
        last_signed_in_at: decode_timestamp(row.try_get("last_signed_in_at")?)?,
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

fn decode_snapshot(row: sqlx::sqlite::SqliteRow) -> Result<Snapshot, StoreError> {
    Ok(Snapshot {
        id: Id::from_raw(row.try_get("id")?),
        subject_id: Id::from_raw(row.try_get("subject_id")?),
        head: decode_sha(row.try_get("head")?, "head")?,
        base: row
            .try_get::<Option<String>, _>("base")?
            .map(|sha| decode_sha(sha, "base"))
            .transpose()?,
        merge_base: row
            .try_get::<Option<String>, _>("merge_base")?
            .map(|sha| decode_sha(sha, "merge_base"))
            .transpose()?,
        forge: ForgeMetadata {
            title: row.try_get("forge_title")?,
            body: row.try_get("forge_body")?,
            author: row.try_get("forge_author")?,
            url: row.try_get("forge_url")?,
            base_ref: row.try_get("forge_base_ref")?,
            head_ref: row.try_get("forge_head_ref")?,
            state: row
                .try_get::<Option<String>, _>("forge_state")?
                .map(decode_change_state)
                .transpose()?,
            merge_commit: row
                .try_get::<Option<String>, _>("forge_merge_commit")?
                .map(|sha| decode_sha(sha, "forge_merge_commit"))
                .transpose()?,
            people: Vec::new(),
            signature: Signature {
                present: row.try_get::<i64, _>("signature_present")? != 0,
                verified: row
                    .try_get::<Option<i64>, _>("signature_verified")?
                    .map(|value| value != 0),
                signer: row.try_get("signature_signer")?,
                reason: row.try_get("signature_reason")?,
            },
        },
        observed_at: decode_timestamp(row.try_get("observed_at")?)?,
    })
}

fn decode_run(row: sqlx::sqlite::SqliteRow, snapshot_id: Id<Snapshot>) -> Result<Run, StoreError> {
    let status: String = row.try_get("status")?;
    let detail: Option<String> = row.try_get("status_detail")?;

    Ok(Run {
        id: Id::from_raw(row.try_get("id")?),
        snapshot_id,
        analyzer: row.try_get("analyzer")?,
        status: decode_run_status(status, detail)?,
        compared_against: row
            .try_get::<Option<i64>, _>("compared_against")?
            .map(Id::from_raw),
        started_at: decode_timestamp(row.try_get("started_at")?)?,
        finished_at: row
            .try_get::<Option<String>, _>("finished_at")?
            .map(decode_timestamp)
            .transpose()?,
    })
}

fn decode_sha(sha: String, field: &'static str) -> Result<CommitSha, StoreError> {
    CommitSha::new(&sha).map_err(|_| StoreError::Unreadable { field, value: sha })
}

async fn project_of_snapshot(
    connection: &mut SqliteConnection,
    snapshot_id: Id<Snapshot>,
) -> Result<Id<Project>, StoreError> {
    let row = sqlx::query(
        "SELECT s.project_id FROM subjects s \
         JOIN snapshots n ON n.subject_id = s.id WHERE n.id = ?",
    )
    .bind(snapshot_id.raw())
    .fetch_one(&mut *connection)
    .await?;

    Ok(Id::from_raw(row.try_get("project_id")?))
}

async fn resolve_issue(
    connection: &mut SqliteConnection,
    ids: &IdGenerator,
    project_id: Id<Project>,
    analyzer: &str,
    fingerprint: &Fingerprint,
    seen_at: Id<Snapshot>,
) -> Result<Id<Issue>, StoreError> {
    let existing = sqlx::query("SELECT id FROM issues WHERE project_id = ? AND fingerprint = ?")
        .bind(project_id.raw())
        .bind(&fingerprint.hash)
        .fetch_optional(&mut *connection)
        .await?;

    if let Some(row) = existing {
        let id: Id<Issue> = Id::from_raw(row.try_get("id")?);
        sqlx::query("UPDATE issues SET last_seen = ? WHERE id = ?")
            .bind(seen_at.raw())
            .bind(id.raw())
            .execute(&mut *connection)
            .await?;
        return Ok(id);
    }

    let id: Id<Issue> = ids.next();
    let (triage, triage_reason) = encode_triage(&Triage::Untriaged);
    sqlx::query(
        "INSERT INTO issues \
         (id, project_id, analyzer, fingerprint, fingerprint_version, fingerprint_canonical, \
          triage, triage_reason, first_seen, last_seen) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.raw())
    .bind(project_id.raw())
    .bind(analyzer)
    .bind(&fingerprint.hash)
    .bind(i64::from(fingerprint.version))
    .bind(&fingerprint.canonical)
    .bind(triage)
    .bind(triage_reason)
    .bind(seen_at.raw())
    .bind(seen_at.raw())
    .execute(&mut *connection)
    .await?;

    Ok(id)
}

async fn insert_finding(
    connection: &mut SqliteConnection,
    ids: &IdGenerator,
    run_id: Id<Run>,
    issue_id: Id<Issue>,
    finding: &NewFinding,
) -> Result<(), StoreError> {
    let id: Id<Finding> = ids.next();
    let location = encode_location(&finding.location);

    sqlx::query(
        "INSERT INTO findings \
         (id, run_id, issue_id, location_kind, file_path, line_start, line_end, ecosystem, \
          package_name, package_version, severity, confidence, attribution, movement, title, detail) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.raw())
    .bind(run_id.raw())
    .bind(issue_id.raw())
    .bind(location.kind)
    .bind(location.file_path)
    .bind(location.line_start)
    .bind(location.line_end)
    .bind(location.ecosystem)
    .bind(location.package_name)
    .bind(location.package_version)
    .bind(encode_severity(finding.severity))
    .bind(f64::from(finding.confidence.value()))
    .bind(encode_attribution(finding.attribution))
    .bind(finding.movement.map(encode_movement))
    .bind(&finding.title)
    .bind(&finding.detail)
    .execute(&mut *connection)
    .await?;

    Ok(())
}

async fn insert_signal(
    connection: &mut SqliteConnection,
    ids: &IdGenerator,
    run_id: Id<Run>,
    signal: &NewSignal,
) -> Result<(), StoreError> {
    let id: Id<crate::signal::Signal> = ids.next();
    let (kind, score, flag, count) = match signal.value {
        SignalValue::Score(score) => ("score", Some(f64::from(score.value())), None, None),
        SignalValue::Flag(flag) => ("flag", None, Some(flag), None),
        SignalValue::Count(count) => ("count", None, None, Some(count as i64)),
    };

    sqlx::query(
        "INSERT INTO signals \
         (id, run_id, signal_key, value_kind, score, flag, count_value, confidence, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.raw())
    .bind(run_id.raw())
    .bind(encode_signal_key(signal.key))
    .bind(kind)
    .bind(score)
    .bind(flag)
    .bind(count)
    .bind(f64::from(signal.confidence.value()))
    .bind(&signal.reason)
    .execute(&mut *connection)
    .await?;

    Ok(())
}

struct EncodedLocation {
    kind: &'static str,
    file_path: Option<String>,
    line_start: Option<i64>,
    line_end: Option<i64>,
    ecosystem: Option<&'static str>,
    package_name: Option<String>,
    package_version: Option<String>,
}

fn encode_location(location: &Location) -> EncodedLocation {
    match location {
        Location::File { path, span } => EncodedLocation {
            kind: "file",
            file_path: Some(path.as_str().to_owned()),
            line_start: span.map(|span| i64::from(span.start)),
            line_end: span.map(|span| i64::from(span.end)),
            ecosystem: None,
            package_name: None,
            package_version: None,
        },
        Location::Package {
            path,
            ecosystem,
            name,
            version,
        } => EncodedLocation {
            kind: "package",
            file_path: Some(path.as_str().to_owned()),
            line_start: None,
            line_end: None,
            ecosystem: Some(encode_ecosystem(*ecosystem)),
            package_name: Some(name.clone()),
            package_version: Some(version.clone()),
        },
    }
}

fn decode_location(row: &sqlx::sqlite::SqliteRow) -> Result<Location, StoreError> {
    let kind: String = row.try_get("location_kind")?;
    match kind.as_str() {
        "file" => {
            let start: Option<i64> = row.try_get("line_start")?;
            let end: Option<i64> = row.try_get("line_end")?;
            Ok(Location::File {
                path: decode_path(row)?,
                span: start.zip(end).map(|(start, end)| LineSpan {
                    start: start as u32,
                    end: end as u32,
                }),
            })
        }
        "package" => Ok(Location::Package {
            path: decode_path(row)?,
            ecosystem: decode_ecosystem(row.try_get("ecosystem")?)?,
            name: row.try_get("package_name")?,
            version: row.try_get("package_version")?,
        }),
        other => Err(StoreError::Unreadable {
            field: "location_kind",
            value: other.to_owned(),
        }),
    }
}

fn decode_path(row: &sqlx::sqlite::SqliteRow) -> Result<RepoPath, StoreError> {
    let path: String = row.try_get("file_path")?;
    RepoPath::new(&path).map_err(|_| StoreError::Unreadable {
        field: "file_path",
        value: path,
    })
}

fn decode_subject(row: &sqlx::sqlite::SqliteRow) -> Result<Subject, StoreError> {
    Ok(Subject {
        id: Id::from_raw(row.try_get("subject_id")?),
        project_id: Id::from_raw(row.try_get("project_id")?),
        kind: decode_subject_kind(row.try_get("subject_kind")?, row.try_get("subject_key")?)?,
    })
}

fn decode_subject_kind(kind: String, key: String) -> Result<SubjectKind, StoreError> {
    match kind.as_str() {
        "change" => key
            .parse()
            .map(|number| SubjectKind::Change { number })
            .map_err(|_| StoreError::Unreadable {
                field: "subject_key",
                value: key,
            }),
        "branch" => Ok(SubjectKind::Branch { name: key }),
        "commit" => decode_sha(key, "subject_key").map(|sha| SubjectKind::Commit { sha }),
        _ => Err(StoreError::Unreadable {
            field: "subject_kind",
            value: kind,
        }),
    }
}

fn encode_subject_kind(kind: &SubjectKind) -> (&'static str, String) {
    match kind {
        SubjectKind::Change { number } => ("change", number.to_string()),
        SubjectKind::Branch { name } => ("branch", name.clone()),
        SubjectKind::Commit { sha } => ("commit", sha.to_string()),
    }
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

fn encode_change_state(state: ChangeState) -> &'static str {
    match state {
        ChangeState::Open => "open",
        ChangeState::Closed => "closed",
        ChangeState::Merged => "merged",
    }
}

fn decode_change_state(state: String) -> Result<ChangeState, StoreError> {
    match state.as_str() {
        "open" => Ok(ChangeState::Open),
        "closed" => Ok(ChangeState::Closed),
        "merged" => Ok(ChangeState::Merged),
        _ => Err(StoreError::Unreadable {
            field: "forge_state",
            value: state,
        }),
    }
}

fn encode_run_status(status: &RunStatus) -> (&'static str, Option<String>) {
    match status {
        RunStatus::Running => ("running", None),
        RunStatus::Succeeded => ("succeeded", None),
        RunStatus::Failed { message } => ("failed", Some(message.clone())),
        RunStatus::TimedOut => ("timed_out", None),
        RunStatus::Skipped { reason } => ("skipped", Some(reason.clone())),
    }
}

fn decode_run_status(status: String, detail: Option<String>) -> Result<RunStatus, StoreError> {
    match (status.as_str(), detail) {
        ("running", _) => Ok(RunStatus::Running),
        ("succeeded", _) => Ok(RunStatus::Succeeded),
        ("failed", Some(message)) => Ok(RunStatus::Failed { message }),
        ("timed_out", _) => Ok(RunStatus::TimedOut),
        ("skipped", Some(reason)) => Ok(RunStatus::Skipped { reason }),
        _ => Err(StoreError::Unreadable {
            field: "run_status",
            value: status,
        }),
    }
}

fn encode_check_status(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Queued => "queued",
        CheckStatus::InProgress => "in_progress",
        CheckStatus::Completed => "completed",
    }
}

fn decode_check_status(status: String) -> Result<CheckStatus, StoreError> {
    match status.as_str() {
        "queued" => Ok(CheckStatus::Queued),
        "in_progress" => Ok(CheckStatus::InProgress),
        "completed" => Ok(CheckStatus::Completed),
        _ => Err(StoreError::Unreadable {
            field: "check_run_status",
            value: status,
        }),
    }
}

fn encode_check_conclusion(conclusion: CheckConclusion) -> &'static str {
    match conclusion {
        CheckConclusion::Success => "success",
        CheckConclusion::Failure => "failure",
        CheckConclusion::Neutral => "neutral",
        CheckConclusion::Skipped => "skipped",
        CheckConclusion::Cancelled => "cancelled",
        CheckConclusion::TimedOut => "timed_out",
        CheckConclusion::ActionRequired => "action_required",
        CheckConclusion::Stale => "stale",
    }
}

fn decode_check_conclusion(conclusion: String) -> Result<CheckConclusion, StoreError> {
    match conclusion.as_str() {
        "success" => Ok(CheckConclusion::Success),
        "failure" => Ok(CheckConclusion::Failure),
        "neutral" => Ok(CheckConclusion::Neutral),
        "skipped" => Ok(CheckConclusion::Skipped),
        "cancelled" => Ok(CheckConclusion::Cancelled),
        "timed_out" => Ok(CheckConclusion::TimedOut),
        "action_required" => Ok(CheckConclusion::ActionRequired),
        "stale" => Ok(CheckConclusion::Stale),
        _ => Err(StoreError::Unreadable {
            field: "check_run_conclusion",
            value: conclusion,
        }),
    }
}

fn encode_triage(triage: &Triage) -> (&'static str, Option<String>) {
    match triage {
        Triage::Untriaged => ("untriaged", None),
        Triage::Acknowledged => ("acknowledged", None),
        Triage::Muted { reason } => ("muted", Some(reason.clone())),
    }
}

fn decode_triage(triage: String, reason: Option<String>) -> Result<Triage, StoreError> {
    match (triage.as_str(), reason) {
        ("untriaged", _) => Ok(Triage::Untriaged),
        ("acknowledged", _) => Ok(Triage::Acknowledged),
        ("muted", Some(reason)) => Ok(Triage::Muted { reason }),
        _ => Err(StoreError::Unreadable {
            field: "triage",
            value: triage,
        }),
    }
}

fn encode_severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

fn decode_severity(severity: String) -> Result<Severity, StoreError> {
    match severity.as_str() {
        "info" => Ok(Severity::Info),
        "low" => Ok(Severity::Low),
        "medium" => Ok(Severity::Medium),
        "high" => Ok(Severity::High),
        "critical" => Ok(Severity::Critical),
        _ => Err(StoreError::Unreadable {
            field: "severity",
            value: severity,
        }),
    }
}

fn encode_attribution(attribution: Attribution) -> &'static str {
    match attribution {
        Attribution::Introduced => "introduced",
        Attribution::Preexisting => "preexisting",
        Attribution::Unknown => "unknown",
    }
}

fn decode_attribution(attribution: String) -> Result<Attribution, StoreError> {
    match attribution.as_str() {
        "introduced" => Ok(Attribution::Introduced),
        "preexisting" => Ok(Attribution::Preexisting),
        "unknown" => Ok(Attribution::Unknown),
        _ => Err(StoreError::Unreadable {
            field: "attribution",
            value: attribution,
        }),
    }
}

fn encode_ecosystem(ecosystem: Ecosystem) -> &'static str {
    match ecosystem {
        Ecosystem::Cargo => "cargo",
        Ecosystem::Npm => "npm",
        Ecosystem::Nix => "nix",
    }
}

fn decode_ecosystem(ecosystem: String) -> Result<Ecosystem, StoreError> {
    match ecosystem.as_str() {
        "cargo" => Ok(Ecosystem::Cargo),
        "npm" => Ok(Ecosystem::Npm),
        "nix" => Ok(Ecosystem::Nix),
        _ => Err(StoreError::Unreadable {
            field: "ecosystem",
            value: ecosystem,
        }),
    }
}

fn encode_signal_key(key: SignalKey) -> &'static str {
    match key {
        SignalKey::OffTask => "off_task",
        SignalKey::DiffSize => "diff_size",
        SignalKey::BlastRadius => "blast_radius",
        SignalKey::DependencyRisk => "dependency_risk",
        SignalKey::TestsFailing => "tests_failing",
        SignalKey::LinksAdded => "links_added",
        SignalKey::RepositoryHygiene => "repository_hygiene",
    }
}

fn decode_signal_key(key: String) -> Result<SignalKey, StoreError> {
    match key.as_str() {
        "off_task" => Ok(SignalKey::OffTask),
        "diff_size" => Ok(SignalKey::DiffSize),
        "blast_radius" => Ok(SignalKey::BlastRadius),
        "dependency_risk" => Ok(SignalKey::DependencyRisk),
        "tests_failing" => Ok(SignalKey::TestsFailing),
        "links_added" => Ok(SignalKey::LinksAdded),
        "repository_hygiene" => Ok(SignalKey::RepositoryHygiene),
        _ => Err(StoreError::Unreadable {
            field: "signal_key",
            value: key,
        }),
    }
}

fn decode_signal_value(row: &sqlx::sqlite::SqliteRow) -> Result<SignalValue, StoreError> {
    let kind: String = row.try_get("value_kind")?;
    match kind.as_str() {
        "score" => {
            let Some(score) = row.try_get::<Option<f64>, _>("score")? else {
                return Err(StoreError::Unreadable {
                    field: "signal_score",
                    value: "missing".to_owned(),
                });
            };
            Score::new(score as f32)
                .map(SignalValue::Score)
                .ok_or(StoreError::Unreadable {
                    field: "signal_score",
                    value: score.to_string(),
                })
        }
        "flag" => {
            let Some(flag) = row.try_get::<Option<bool>, _>("flag")? else {
                return Err(StoreError::Unreadable {
                    field: "signal_flag",
                    value: "missing".to_owned(),
                });
            };
            Ok(SignalValue::Flag(flag))
        }
        "count" => {
            let Some(count) = row.try_get::<Option<i64>, _>("count_value")? else {
                return Err(StoreError::Unreadable {
                    field: "signal_count",
                    value: "missing".to_owned(),
                });
            };
            u64::try_from(count)
                .map(SignalValue::Count)
                .map_err(|_| StoreError::Unreadable {
                    field: "signal_count",
                    value: count.to_string(),
                })
        }
        _ => Err(StoreError::Unreadable {
            field: "signal_value_kind",
            value: kind,
        }),
    }
}

fn decode_confidence(confidence: f64) -> Result<Confidence, StoreError> {
    Confidence::new(confidence as f32).ok_or(StoreError::Unreadable {
        field: "confidence",
        value: confidence.to_string(),
    })
}

fn decode_timestamp(timestamp: String) -> Result<Timestamp, StoreError> {
    timestamp.parse().map_err(|_| StoreError::Unreadable {
        field: "timestamp",
        value: timestamp,
    })
}

fn encode_movement(movement: VersionMovement) -> &'static str {
    match movement {
        VersionMovement::Added => "added",
        VersionMovement::Removed => "removed",
        VersionMovement::Upgraded => "upgraded",
        VersionMovement::Downgraded => "downgraded",
        VersionMovement::Changed => "changed",
    }
}

fn decode_movement(movement: Option<String>) -> Result<Option<VersionMovement>, StoreError> {
    movement
        .map(|movement| match movement.as_str() {
            "added" => Ok(VersionMovement::Added),
            "removed" => Ok(VersionMovement::Removed),
            "upgraded" => Ok(VersionMovement::Upgraded),
            "downgraded" => Ok(VersionMovement::Downgraded),
            "changed" => Ok(VersionMovement::Changed),
            _ => Err(StoreError::Unreadable {
                field: "movement",
                value: movement,
            }),
        })
        .transpose()
}

fn encode_person_role(role: PersonRole) -> &'static str {
    match role {
        PersonRole::Author => "author",
        PersonRole::Committer => "committer",
        PersonRole::CoAuthor => "co_author",
        PersonRole::SignedOffBy => "signed_off_by",
        PersonRole::Submitter => "submitter",
        PersonRole::Reviewer => "reviewer",
    }
}

fn decode_person_role(role: String) -> Result<PersonRole, StoreError> {
    match role.as_str() {
        "author" => Ok(PersonRole::Author),
        "committer" => Ok(PersonRole::Committer),
        "co_author" => Ok(PersonRole::CoAuthor),
        "signed_off_by" => Ok(PersonRole::SignedOffBy),
        "submitter" => Ok(PersonRole::Submitter),
        "reviewer" => Ok(PersonRole::Reviewer),
        _ => Err(StoreError::Unreadable {
            field: "role",
            value: role,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::compare::compare;
    use crate::finding::fingerprint::Components;
    use crate::analysis::{lockfile, runner, secret};

    async fn store() -> Store {
        Store::open("sqlite::memory:", 0).await.expect("opens")
    }

    async fn member(store: &Store) -> User {
        let user = store
            .register_user("https://github.com", "member", "Member", true)
            .await
            .expect("registers")
            .expect("allows user");
        let backup_admin = store
            .register_user("https://github.com", "backup-admin", "Backup Admin", true)
            .await
            .expect("registers")
            .expect("allows backup administrator");
        assert_eq!(
            store
                .set_user_role(backup_admin.id, UserRole::Admin)
                .await
                .expect("promotes backup administrator"),
            UserRoleChange::Updated
        );
        assert_eq!(
            store
                .set_user_role(user.id, UserRole::Member)
                .await
                .expect("sets role"),
            UserRoleChange::Updated
        );
        store.user(user.id).await.expect("reads").expect("user exists")
    }

    fn sha(leading: char) -> CommitSha {
        CommitSha::new(&String::from_iter(std::iter::repeat_n(leading, 40))).expect("valid sha")
    }

    fn new_finding(path: &str, title: &str) -> NewFinding {
        let location = Location::File {
            path: RepoPath::new(path).expect("valid path"),
            span: Some(LineSpan { start: 1, end: 2 }),
        };
        NewFinding {
            movement: None,
            fingerprint: Fingerprint::compute(&Components {
                analyzer: "secret-scan",
                rule: "aws-access-key",
                location: &location,
                title,
                occurrence: 0,
            }),
            location,
            severity: Severity::High,
            confidence: Confidence::new(0.9).expect("in range"),
            attribution: Attribution::Introduced,
            title: title.to_owned(),
            detail: "detail".to_owned(),
        }
    }

    async fn subject_with_snapshots(
        store: &Store,
        count: usize,
    ) -> (Id<Subject>, Vec<Id<Snapshot>>) {
        let project = store
            .create_project(
                member(store).await.id,
                "error-menu",
                RemoteUrl::new("ssh://example.invalid/repo").expect("valid url"),
                ForgeKind::Auto,
                false,
                &[],
            )
            .await
            .expect("project");
        let subject = store
            .upsert_subject(project.id, SubjectKind::Change { number: 42 })
            .await
            .expect("subject");

        let mut snapshots = Vec::new();
        for index in 0..count {
            let head = sha(char::from_digit(index as u32 + 1, 16).expect("digit"));
            let snapshot = store
                .record_snapshot(subject.id, head, None, None, ForgeMetadata::default())
                .await
                .expect("snapshot");
            snapshots.push(snapshot.id);
        }

        (subject.id, snapshots)
    }

    #[tokio::test]
    async fn migration_keeps_existing_projects_in_custom_mode() {
        let store = store().await;
        let project_id = Id::from_raw(42);
        sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES (?, ?, ?)")
            .bind(project_id.raw())
            .bind("existing")
            .bind("ssh://example.invalid/existing")
            .execute(&store.pool)
            .await
            .expect("inserts existing project");
        for analyzer in [lockfile::ANALYZER, secret::ANALYZER] {
            sqlx::query("INSERT INTO project_analyzers (project_id, analyzer) VALUES (?, ?)")
                .bind(project_id.raw())
                .bind(analyzer)
                .execute(&store.pool)
                .await
                .expect("inserts custom analyzer");
        }

        let project = store
            .project(project_id)
            .await
            .expect("reads")
            .expect("exists");

        assert!(!project.uses_default_analyzers);
        assert_eq!(
            store
                .effective_project_analyzers(&project)
                .await
                .expect("resolves"),
            [lockfile::ANALYZER, secret::ANALYZER]
        );
    }

    #[tokio::test]
    async fn default_mode_ignores_the_saved_custom_analyzer_list() {
        let store = store().await;
        let project = store
            .create_project(
                member(&store).await.id,
                "defaults",
                RemoteUrl::new("ssh://example.invalid/defaults").expect("valid url"),
                ForgeKind::Auto,
                true,
                &[secret::ANALYZER],
            )
            .await
            .expect("creates project");

        assert_eq!(
            store
                .effective_project_analyzers(&project)
                .await
                .expect("resolves"),
            runner::DEFAULT_ANALYZERS.map(str::to_owned)
        );
        assert_eq!(
            store
                .custom_project_analyzers(project.id)
                .await
                .expect("reads custom list"),
            [secret::ANALYZER]
        );
    }

    #[tokio::test]
    async fn changing_to_custom_mode_uses_the_new_selected_analyzers() {
        let store = store().await;
        let project = store
            .create_project(
                member(&store).await.id,
                "custom",
                RemoteUrl::new("ssh://example.invalid/custom").expect("valid url"),
                ForgeKind::Auto,
                true,
                &[],
            )
            .await
            .expect("creates project");

        store
            .set_project_analyzers(project.id, false, &[secret::ANALYZER])
            .await
            .expect("updates settings");
        let project = store
            .project(project.id)
            .await
            .expect("reads")
            .expect("exists");

        assert!(!project.uses_default_analyzers);
        assert_eq!(
            store
                .effective_project_analyzers(&project)
                .await
                .expect("resolves"),
            [secret::ANALYZER]
        );
    }

    #[tokio::test]
    async fn unknown_analyzers_are_rejected() {
        let store = store().await;
        let result = store
            .create_project(
                member(&store).await.id,
                "invalid",
                RemoteUrl::new("ssh://example.invalid/invalid").expect("valid url"),
                ForgeKind::Auto,
                false,
                &["unknown"],
            )
            .await;

        assert!(matches!(result, Err(StoreError::UnknownAnalyzer(analyzer)) if analyzer == "unknown"));
    }

    #[tokio::test]
    async fn a_recorded_run_reads_back_whole() {
        let store = store().await;
        let (_, snapshots) = subject_with_snapshots(&store, 1).await;
        let findings = vec![new_finding("src/a.rs", "hardcoded key")];

        let run = store
            .record_run(RunRecord {
                snapshot_id: snapshots[0],
                analyzer: "secret-scan",
                status: RunStatus::Succeeded,
                compared_against: None,
                findings: &findings,
                signals: &[NewSignal {
                    key: SignalKey::LinksAdded,
                    value: SignalValue::Count(3),
                    confidence: Confidence::new(1.0).expect("in range"),
                    reason: "three new links".to_owned(),
                }],
            })
            .await
            .expect("records");

        let stored = store.findings_for_run(run.id).await.expect("reads");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].title, "hardcoded key");
        assert_eq!(stored[0].severity, Severity::High);
        assert_eq!(stored[0].attribution, Attribution::Introduced);
        assert_eq!(stored[0].fingerprint, findings[0].fingerprint);
        assert_eq!(stored[0].location, findings[0].location);
    }

    #[tokio::test]
    async fn the_same_fingerprint_keeps_one_issue_across_snapshots() {
        let store = store().await;
        let (_, snapshots) = subject_with_snapshots(&store, 2).await;
        let findings = vec![new_finding("src/a.rs", "hardcoded key")];

        let mut issue_ids = Vec::new();
        for snapshot in &snapshots {
            let run = store
                .record_run(RunRecord {
                    snapshot_id: *snapshot,
                    analyzer: "secret-scan",
                    status: RunStatus::Succeeded,
                    compared_against: None,
                    findings: &findings,
                    signals: &[],
                })
                .await
                .expect("records");
            let stored = store.findings_for_run(run.id).await.expect("reads");
            issue_ids.push(stored[0].issue_id);
        }

        assert_eq!(issue_ids[0], issue_ids[1]);

        let issue = store
            .issue(issue_ids[0])
            .await
            .expect("reads")
            .expect("exists");
        assert_eq!(issue.first_seen, snapshots[0]);
        assert_eq!(issue.last_seen, snapshots[1]);
        assert_eq!(issue.triage, Triage::Untriaged);
        assert_eq!(issue.analyzer, "secret-scan");
    }

    #[tokio::test]
    async fn different_fingerprints_get_different_issues() {
        let store = store().await;
        let (_, snapshots) = subject_with_snapshots(&store, 1).await;
        let findings = vec![
            new_finding("src/a.rs", "hardcoded key"),
            new_finding("src/b.rs", "hardcoded key"),
        ];

        let run = store
            .record_run(RunRecord {
                snapshot_id: snapshots[0],
                analyzer: "secret-scan",
                status: RunStatus::Succeeded,
                compared_against: None,
                findings: &findings,
                signals: &[],
            })
            .await
            .expect("records");

        let stored = store.findings_for_run(run.id).await.expect("reads");
        assert_ne!(stored[0].issue_id, stored[1].issue_id);
    }

    #[tokio::test]
    async fn history_across_three_runs_reads_as_new_existing_and_resolved() {
        let store = store().await;
        let (subject, snapshots) = subject_with_snapshots(&store, 3).await;

        let first = vec![new_finding("src/a.rs", "hardcoded key")];
        let second = vec![
            new_finding("src/a.rs", "hardcoded key"),
            new_finding("src/b.rs", "hardcoded key"),
        ];
        let third = vec![new_finding("src/b.rs", "hardcoded key")];

        let mut runs = Vec::new();
        for (snapshot, findings) in snapshots.iter().zip([&first, &second, &third]) {
            let run = store
                .record_run(RunRecord {
                    snapshot_id: *snapshot,
                    analyzer: "secret-scan",
                    status: RunStatus::Succeeded,
                    compared_against: None,
                    findings,
                    signals: &[],
                })
                .await
                .expect("records");
            runs.push(run.id);
        }

        let baseline = store
            .last_successful_analyzer(subject, "secret-scan", snapshots[2])
            .await
            .expect("reads")
            .expect("a baseline exists");
        assert_eq!(baseline.id, runs[1]);

        let previous = store.findings_for_run(baseline.id).await.expect("reads");
        let current = store.findings_for_run(runs[2]).await.expect("reads");
        let comparison = compare(&previous, &current);

        assert_eq!(comparison.existing.len(), 1);
        assert_eq!(comparison.existing[0].title, "hardcoded key");
        assert_eq!(comparison.resolved.len(), 1);
        assert!(comparison.new.is_empty());
    }

    #[tokio::test]
    async fn a_failed_run_is_not_a_baseline() {
        let store = store().await;
        let (subject, snapshots) = subject_with_snapshots(&store, 2).await;
        let findings = vec![new_finding("src/a.rs", "hardcoded key")];

        let succeeded = store
            .record_run(RunRecord {
                snapshot_id: snapshots[0],
                analyzer: "secret-scan",
                status: RunStatus::Succeeded,
                compared_against: None,
                findings: &findings,
                signals: &[],
            })
            .await
            .expect("records");

        store
            .record_run(RunRecord {
                snapshot_id: snapshots[1],
                analyzer: "secret-scan",
                status: RunStatus::Failed {
                    message: "parser crashed".to_owned(),
                },
                compared_against: None,
                findings: &[],
                signals: &[],
            })
            .await
            .expect("records");

        let baseline = store
            .last_successful_analyzer(subject, "secret-scan", snapshots[1])
            .await
            .expect("reads")
            .expect("a baseline exists");
        assert_eq!(baseline.id, succeeded.id);
    }

    #[tokio::test]
    async fn a_subject_is_not_duplicated() {
        let store = store().await;
        let project = store
            .create_project(
                member(&store).await.id,
                "error-menu",
                RemoteUrl::new("ssh://example.invalid/repo").expect("valid url"),
                ForgeKind::Auto,
                false,
                &[],
            )
            .await
            .expect("project");

        let first = store
            .upsert_subject(project.id, SubjectKind::Change { number: 42 })
            .await
            .expect("subject");
        let second = store
            .upsert_subject(project.id, SubjectKind::Change { number: 42 })
            .await
            .expect("subject");

        assert_eq!(first.id, second.id);
    }
    #[tokio::test]
    async fn check_run_facts_round_trip_with_urls_and_log_references() {
        let store = store().await;
        let (_, snapshots) = subject_with_snapshots(&store, 1).await;
        let check_runs = [CheckRun {
            name: "unit".to_owned(),
            status: CheckStatus::Completed,
            conclusion: Some(CheckConclusion::Failure),
            url: Some("https://ci.example.test/unit".to_owned()),
            log_excerpt_ref: Some("https://ci.example.test/unit/log".to_owned()),
        }];

        store
            .replace_check_runs(snapshots[0], &check_runs)
            .await
            .expect("records");

        assert_eq!(
            store
                .check_runs_for_snapshot(snapshots[0])
                .await
                .expect("reads"),
            check_runs
        );
    }

    async fn project(store: &Store, name: &str) -> Id<Project> {
        store
            .create_project(
                member(store).await.id,
                name,
                RemoteUrl::new(&format!("ssh://example.invalid/{name}")).expect("valid url"),
                ForgeKind::Auto,
                true,
                &[],
            )
            .await
            .expect("project")
            .id
    }

    #[tokio::test]
    async fn a_claimed_job_is_not_offered_twice_and_a_retry_waits_its_turn() {
        let store = store().await;
        let project_id = project(&store, "queued").await;

        let job_id = store
            .enqueue(project_id, JobKind::Discover)
            .await
            .expect("enqueues")
            .expect("a new job");
        // A project already waiting does not need a second job.
        assert!(
            store
                .enqueue(project_id, JobKind::Discover)
                .await
                .expect("enqueues")
                .is_none()
        );

        let claimed = store
            .claim("worker-a", queue::LEASE)
            .await
            .expect("claims")
            .expect("a job");
        assert_eq!(claimed.id, job_id);
        assert_eq!(claimed.attempts, 1);
        assert!(
            store
                .claim("worker-b", queue::LEASE)
                .await
                .expect("claims")
                .is_none()
        );

        let later = Timestamp::now() + std::time::Duration::from_secs(120);
        store
            .fail(job_id, "forge said no", Some(later))
            .await
            .expect("fails");
        // Queued again, but not yet: its retry time has not arrived.
        assert!(
            store
                .claim("worker-a", queue::LEASE)
                .await
                .expect("claims")
                .is_none()
        );

        store
            .fail(job_id, "forge said no", None)
            .await
            .expect("fails");
        assert!(
            store
                .claim("worker-a", queue::LEASE)
                .await
                .expect("claims")
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_expired_lease_returns_the_job_to_the_queue() {
        let store = store().await;
        let project_id = project(&store, "leased").await;

        store
            .enqueue(project_id, JobKind::Discover)
            .await
            .expect("enqueues");
        let claimed = store
            .claim("worker-a", std::time::Duration::ZERO)
            .await
            .expect("claims")
            .expect("a job");

        assert_eq!(store.reclaim_expired_leases().await.expect("reclaims"), 1);

        let again = store
            .claim("worker-b", queue::LEASE)
            .await
            .expect("claims")
            .expect("a job");
        assert_eq!(again.id, claimed.id);
        assert_eq!(again.attempts, 2);
    }

    #[tokio::test]
    async fn a_project_is_due_once_per_interval() {
        let store = store().await;
        let project_id = project(&store, "scheduled").await;
        let now = Timestamp::now();

        assert_eq!(store.projects_due(now).await.expect("reads"), [project_id]);

        store.mark_enqueued(project_id, now).await.expect("marks");
        assert!(store.projects_due(now).await.expect("reads").is_empty());

        // The default interval is fifteen minutes.
        let later = now + std::time::Duration::from_secs(900);
        assert_eq!(store.projects_due(later).await.expect("reads"), [project_id]);
    }
    #[tokio::test]
    async fn registration_provisions_guests_and_changes_instance_roles() {
        let store = store().await;
        let admin = store
            .register_user("https://issuer.example", "admin", "Admin", true)
            .await
            .expect("registers")
            .expect("registration allowed");
        assert_eq!(admin.role, UserRole::Admin);

        let guest = store
            .register_user("https://issuer.example", "guest", "Guest", true)
            .await
            .expect("registers")
            .expect("guest provisioning allowed");
        assert_eq!(guest.role, UserRole::Guest);

        let signed_in_guest = store
            .register_user("https://issuer.example", "guest", "Updated guest", false)
            .await
            .expect("signs in")
            .expect("existing user is allowed");
        assert_eq!(signed_in_guest.role, UserRole::Guest);
        assert_eq!(signed_in_guest.display_name, "Updated guest");

        assert_eq!(
            store
                .set_user_role(guest.id, UserRole::Member)
                .await
                .expect("changes role"),
            UserRoleChange::Updated
        );
        assert_eq!(
            store.user(guest.id).await.expect("reads").expect("user exists").role,
            UserRole::Member
        );
        assert_eq!(
            store
                .set_user_role(Id::from_raw(42), UserRole::Admin)
                .await
                .expect("checks missing user"),
            UserRoleChange::Missing
        );
        assert_eq!(
            store
                .list_users()
                .await
                .expect("lists users")
                .into_iter()
                .map(|user| user.role)
                .collect::<Vec<_>>(),
            [UserRole::Admin, UserRole::Member]
        );

        assert!(store
            .register_user("https://issuer.example", "closed", "Closed", false)
            .await
            .expect("checks registration")
            .is_none());
    }

    #[tokio::test]
    async fn the_final_administrator_cannot_be_demoted() {
        let store = store().await;
        let admin = store
            .register_user("https://issuer.example", "admin", "Admin", true)
            .await
            .expect("registers")
            .expect("allows first user");

        assert_eq!(
            store
                .set_user_role(admin.id, UserRole::Member)
                .await
                .expect("protects administrator"),
            UserRoleChange::FinalAdmin
        );
        assert_eq!(
            store.user(admin.id).await.expect("reads").expect("user exists").role,
            UserRole::Admin
        );
    }

    #[tokio::test]
    async fn authentication_expiry_uses_numeric_time_and_removes_expired_rows() {
        let store = store().await;
        let expiry = Timestamp::from_second(1).expect("valid timestamp");
        let now = expiry + std::time::Duration::from_millis(500);

        store
            .create_auth_attempt("expired", expiry)
            .await
            .expect("stores attempt");
        assert!(!store
            .consume_auth_attempt("expired", now)
            .await
            .expect("consumes expired attempt"));

        let user = store
            .register_user("https://github.com", "1", "luc", true)
            .await
            .expect("registers")
            .expect("allows user");
        store
            .create_session(user.id, "expired", expiry)
            .await
            .expect("stores session");
        assert!(store
            .user_for_session("expired", now)
            .await
            .expect("reads session")
            .is_none());
    }

    #[tokio::test]
    async fn project_roles_follow_instance_access_and_membership() {
        let store = store().await;
        let admin = store
            .register_user("https://github.com", "admin", "Admin", true)
            .await
            .expect("registers")
            .expect("allows user");
        let guest = store
            .register_user("https://github.com", "guest", "Guest", true)
            .await
            .expect("registers")
            .expect("allows user");
        let owner = member(&store).await;
        let viewer = store
            .register_user("https://github.com", "viewer", "Viewer", true)
            .await
            .expect("registers")
            .expect("allows user");
        assert_eq!(
            store
                .set_user_role(viewer.id, UserRole::Member)
                .await
                .expect("sets viewer role"),
            UserRoleChange::Updated
        );
        let viewer = store.user(viewer.id).await.expect("reads").expect("user exists");

        let project = store
            .create_project(
                owner.id,
                "error-menu",
                RemoteUrl::new("ssh://example.invalid/error-menu").expect("valid url"),
                ForgeKind::Auto,
                false,
                &[],
            )
            .await
            .expect("creates project");
        store
            .set_project_member(project.id, guest.id, ProjectRole::Viewer)
            .await
            .expect("adds guest membership");
        store
            .set_project_member(project.id, viewer.id, ProjectRole::Viewer)
            .await
            .expect("adds viewer");

        assert_eq!(
            store.project_role_for(&admin, project.id).await.expect("resolves"),
            Some(ProjectRole::Owner)
        );
        assert_eq!(
            store.project_role_for(&guest, project.id).await.expect("resolves"),
            None
        );
        assert_eq!(
            store.project_role_for(&owner, project.id).await.expect("resolves"),
            Some(ProjectRole::Owner)
        );
        assert_eq!(
            store.project_role_for(&viewer, project.id).await.expect("resolves"),
            Some(ProjectRole::Viewer)
        );
        assert!(store
            .list_projects_for(&guest)
            .await
            .expect("lists projects")
            .is_empty());
        for user in [&admin, &owner, &viewer] {
            assert_eq!(
                store
                    .list_projects_for(user)
                    .await
                    .expect("lists projects")
                    .into_iter()
                    .map(|project| project.id)
                    .collect::<Vec<_>>(),
                [project.id]
            );
        }

        let members = store
            .list_project_members(project.id)
            .await
            .expect("lists members");
        assert_eq!(
            members,
            [
                ProjectMember {
                    user_id: guest.id,
                    display_name: "Guest".to_owned(),
                    role: ProjectRole::Viewer,
                },
                ProjectMember {
                    user_id: owner.id,
                    display_name: "Member".to_owned(),
                    role: ProjectRole::Owner,
                },
                ProjectMember {
                    user_id: viewer.id,
                    display_name: "Viewer".to_owned(),
                    role: ProjectRole::Viewer,
                },
            ]
        );

        store
            .set_project_member(project.id, viewer.id, ProjectRole::Operator)
            .await
            .expect("updates role");
        assert_eq!(
            store.project_role_for(&viewer, project.id).await.expect("resolves"),
            Some(ProjectRole::Operator)
        );
    }

    #[tokio::test]
    async fn deleted_session_stops_resolving_a_user() {
        let store = store().await;
        let user = store
            .register_user("https://github.com", "1", "luc", true)
            .await
            .expect("registers")
            .expect("allows user");
        let now = Timestamp::now();
        store
            .create_session(user.id, "live", now + std::time::Duration::from_secs(60))
            .await
            .expect("stores session");
        assert_eq!(
            store
                .user_for_session("live", now)
                .await
                .expect("reads session")
                .map(|user| user.id),
            Some(user.id)
        );

        assert!(store.delete_session("live").await.expect("deletes session"));
        assert!(store
            .user_for_session("live", now)
            .await
            .expect("reads session")
            .is_none());
        assert!(!store
            .delete_session("live")
            .await
            .expect("reports missing session"));
    }

    #[tokio::test]
    async fn removed_project_member_stops_resolving() {
        let store = store().await;
        let owner = member(&store).await;
        let collaborator = store
            .register_user("https://github.com", "collaborator", "Collaborator", true)
            .await
            .expect("registers")
            .expect("allows user");
        let project = store
            .create_project(
                owner.id,
                "error-menu",
                RemoteUrl::new("ssh://example.invalid/error-menu").expect("valid url"),
                ForgeKind::Auto,
                false,
                &[],
            )
            .await
            .expect("creates project");
        store
            .set_project_member(project.id, collaborator.id, ProjectRole::Operator)
            .await
            .expect("adds collaborator");

        assert_eq!(
            store
                .remove_project_member(project.id, collaborator.id)
                .await
                .expect("removes collaborator"),
            ProjectMemberChange::Removed
        );
        assert!(store
            .project_member(project.id, collaborator.id)
            .await
            .expect("reads membership")
            .is_none());
        assert_eq!(
            store
                .remove_project_member(project.id, collaborator.id)
                .await
                .expect("reports missing membership"),
            ProjectMemberChange::Missing
        );
    }

    #[tokio::test]
    async fn owner_changes_preserve_a_project_owner() {
        let store = store().await;
        let owner = member(&store).await;
        let collaborator = store
            .register_user("https://github.com", "collaborator", "Collaborator", true)
            .await
            .expect("registers")
            .expect("allows user");
        let project = store
            .create_project(
                owner.id,
                "error-menu",
                RemoteUrl::new("ssh://example.invalid/error-menu").expect("valid url"),
                ForgeKind::Auto,
                false,
                &[],
            )
            .await
            .expect("creates project");
        assert_eq!(
            store.count_project_owners(project.id).await.expect("counts owners"),
            1
        );

        store
            .set_project_member(project.id, collaborator.id, ProjectRole::Owner)
            .await
            .expect("promotes collaborator");
        assert_eq!(
            store.count_project_owners(project.id).await.expect("counts owners"),
            2
        );

        store
            .set_project_member(project.id, collaborator.id, ProjectRole::Viewer)
            .await
            .expect("demotes collaborator");
        assert_eq!(
            store.count_project_owners(project.id).await.expect("counts owners"),
            1
        );

        assert_eq!(
            store
                .remove_project_member(project.id, owner.id)
                .await
                .expect("protects owner"),
            ProjectMemberChange::FinalOwner
        );
        assert_eq!(
            store.count_project_owners(project.id).await.expect("counts owners"),
            1
        );
        assert_eq!(
            store
                .set_project_member(project.id, owner.id, ProjectRole::Viewer)
                .await
                .expect("protects owner"),
            ProjectMemberChange::FinalOwner
        );
        assert_eq!(
            store.project_role_for(&owner, project.id).await.expect("reads role"),
            Some(ProjectRole::Owner)
        );
    }

}
