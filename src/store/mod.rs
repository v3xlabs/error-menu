use std::str::FromStr;

use jiff::Timestamp;
use sqlx::error::DatabaseError;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::analysis::ci_checks::{CheckConclusion, CheckRun, CheckStatus};
use crate::analysis::{Run, RunStatus, runner};
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
use crate::user::{ProjectMember, ProjectRole, User, UserRole};
use crate::vcs::{CommitSha, RemoteUrl, RepoPath};
use crate::watch::{Project, ProjectIcon, Snapshot, Subject, SubjectKind};
mod analysis;
mod auth;
mod jobs;
mod projects;
mod snapshots;

pub use auth::UserRoleChange;
pub use projects::ProjectMemberChange;

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
impl StoreError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Database(_) => "database",
            Self::Migration(_) => "migration",
            Self::Unreadable { .. } => "unreadable",
            Self::UnknownAnalyzer(_) => "unknown_analyzer",
        }
    }
    pub fn database_code(&self) -> Option<String> {
        let Self::Database(sqlx::Error::Database(error)) = self else {
            return None;
        };

        error.code().map(|code| code.into_owned())
    }
}

pub struct Store {
    pub(crate) pool: SqlitePool,
    pub(crate) ids: IdGenerator,
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
}

fn decode_timestamp(timestamp: String) -> Result<Timestamp, StoreError> {
    timestamp.parse().map_err(|_| StoreError::Unreadable {
        field: "timestamp",
        value: timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{lockfile, runner, secret};
    use crate::finding::compare::compare;
    use crate::finding::fingerprint::Components;

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
        store
            .user(user.id)
            .await
            .expect("reads")
            .expect("user exists")
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

        assert!(
            matches!(result, Err(StoreError::UnknownAnalyzer(analyzer)) if analyzer == "unknown")
        );
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
        assert_eq!(
            store.projects_due(later).await.expect("reads"),
            [project_id]
        );
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
            store
                .user(guest.id)
                .await
                .expect("reads")
                .expect("user exists")
                .role,
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

        assert!(
            store
                .register_user("https://issuer.example", "closed", "Closed", false)
                .await
                .expect("checks registration")
                .is_none()
        );
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
            store
                .user(admin.id)
                .await
                .expect("reads")
                .expect("user exists")
                .role,
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
        assert!(
            !store
                .consume_auth_attempt("expired", now)
                .await
                .expect("consumes expired attempt")
        );

        let user = store
            .register_user("https://github.com", "1", "luc", true)
            .await
            .expect("registers")
            .expect("allows user");
        store
            .create_session(user.id, "expired", expiry)
            .await
            .expect("stores session");
        assert!(
            store
                .user_for_session("expired", now)
                .await
                .expect("reads session")
                .is_none()
        );
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
        let viewer = store
            .user(viewer.id)
            .await
            .expect("reads")
            .expect("user exists");

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
            store
                .project_role_for(&admin, project.id)
                .await
                .expect("resolves"),
            Some(ProjectRole::Owner)
        );
        assert_eq!(
            store
                .project_role_for(&guest, project.id)
                .await
                .expect("resolves"),
            None
        );
        assert_eq!(
            store
                .project_role_for(&owner, project.id)
                .await
                .expect("resolves"),
            Some(ProjectRole::Owner)
        );
        assert_eq!(
            store
                .project_role_for(&viewer, project.id)
                .await
                .expect("resolves"),
            Some(ProjectRole::Viewer)
        );
        assert!(
            store
                .list_projects_for(&guest)
                .await
                .expect("lists projects")
                .is_empty()
        );
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
            store
                .project_role_for(&viewer, project.id)
                .await
                .expect("resolves"),
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
        assert!(
            store
                .user_for_session("live", now)
                .await
                .expect("reads session")
                .is_none()
        );
        assert!(
            !store
                .delete_session("live")
                .await
                .expect("reports missing session")
        );
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
        assert!(
            store
                .project_member(project.id, collaborator.id)
                .await
                .expect("reads membership")
                .is_none()
        );
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
            store
                .count_project_owners(project.id)
                .await
                .expect("counts owners"),
            1
        );

        store
            .set_project_member(project.id, collaborator.id, ProjectRole::Owner)
            .await
            .expect("promotes collaborator");
        assert_eq!(
            store
                .count_project_owners(project.id)
                .await
                .expect("counts owners"),
            2
        );

        store
            .set_project_member(project.id, collaborator.id, ProjectRole::Viewer)
            .await
            .expect("demotes collaborator");
        assert_eq!(
            store
                .count_project_owners(project.id)
                .await
                .expect("counts owners"),
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
            store
                .count_project_owners(project.id)
                .await
                .expect("counts owners"),
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
            store
                .project_role_for(&owner, project.id)
                .await
                .expect("reads role"),
            Some(ProjectRole::Owner)
        );
    }
}
