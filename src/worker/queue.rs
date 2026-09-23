use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::app::AppState;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::prelude::*;
use crate::registry;
use crate::user::token::ApiToken;
use crate::user::{attempt, session};
use crate::worker::discovery;
use tracing::Instrument;

/// A stopped process loses its claim after this long; active discovery renews it.
pub const LEASE: Duration = Duration::from_secs(900);

/// How often the app looks for projects that are due and jobs that are ready. The interval
/// bounds how late a due project starts, not how often a project is polled.
const TICK: Duration = Duration::from_secs(15);

/// A job that keeps failing stops asking. Three attempts with a widening wait is enough to
/// ride out a forge that is rate limiting or briefly down.
const MAX_ATTEMPTS: i64 = 3;
const RETRY_BACKOFF_SECONDS: i64 = 120;

/// Expired sessions, tokens and login attempts are already invisible to authentication,
/// so removing them is housekeeping and belongs on its own slow schedule.
const AUTH_CLEANUP: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Discover,
    PackageFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Id<Job>,
    pub project_id: Id<Project>,
    pub kind: JobKind,
    pub state: JobState,
    pub attempts: i64,
    pub last_error: Option<String>,
    claimed_by: Option<String>,
    pub available_at: Timestamp,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

impl Job {
    /// Puts a project in the queue unless it is already there. A project that is waiting or
    /// running does not need a second job: the work reads the forge when it starts, so a
    /// queued job already covers anything that changed since it was queued.
    pub async fn enqueue(
        database: &Database,
        project_id: Id<Project>,
        kind: JobKind,
    ) -> Result<Option<Id<Job>>, DatabaseError> {
        let id = database.ids.next::<Job>();
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
        .bind(kind.stored())
        .bind(&now)
        .bind(&now)
        .bind(project_id.raw())
        .bind(kind.stored())
        .execute(&database.pool)
        .await?;

        Ok((inserted.rows_affected() > 0).then_some(id))
    }

    /// Takes the oldest ready job of the highest priority and holds it for the lease. The
    /// claim is one statement, so two workers cannot take the same job.
    pub async fn claim(
        database: &Database,
        worker: &str,
        lease: Duration,
    ) -> Result<Option<Job>, DatabaseError> {
        let now = Timestamp::now();
        let expires = now + lease;
        let claim = format!("{worker}:{}", crate::trace::new_id());
        let row = sqlx::query(
            "UPDATE jobs SET state = 'running', claimed_by = ?, lease_expires_at = ?, \
                    attempts = attempts + 1 \
             WHERE id = ( \
                 SELECT id FROM jobs WHERE state = 'queued' AND available_at <= ? \
                 ORDER BY priority DESC, id LIMIT 1 \
             ) \
             RETURNING id, project_id, kind, state, attempts, claimed_by, last_error, available_at, \
                       created_at, finished_at",
        )
        .bind(&claim)
        .bind(expires.to_string())
        .bind(now.to_string())
        .fetch_optional(&database.pool)
        .await?;

        row.as_ref().map(Job::decode_row).transpose()
    }

    /// A worker that dies mid-job leaves its claim behind. The lease is what makes that
    /// recoverable without a human, so an expired one returns the job to the queue.
    pub async fn reclaim_expired_leases(database: &Database) -> Result<u64, DatabaseError> {
        let affected = sqlx::query(
            "UPDATE jobs SET state = 'queued', claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = 'lease expired' \
             WHERE state = 'running' AND lease_expires_at IS NOT NULL AND lease_expires_at < ?",
        )
        .bind(Timestamp::now().to_string())
        .execute(&database.pool)
        .await?;

        Ok(affected.rows_affected())
    }

    /// The newest jobs, so a reader can see that scanning happens without them. A project
    /// filter answers the question a project page asks.
    pub async fn recent(
        database: &Database,
        project_id: Option<Id<Project>>,
        limit: i64,
    ) -> Result<Vec<Job>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT id, project_id, kind, state, attempts, claimed_by, last_error, available_at, \
                    created_at, finished_at \
             FROM jobs WHERE (?1 IS NULL OR project_id = ?1) ORDER BY id DESC LIMIT ?2",
        )
        .bind(project_id.map(Id::raw))
        .bind(limit)
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(Job::decode_row).collect()
    }

    pub async fn renew(&self, database: &Database, lease: Duration) -> Result<bool, DatabaseError> {
        let now = Timestamp::now();
        let updated = sqlx::query(
            "UPDATE jobs SET lease_expires_at = ? \
             WHERE id = ? AND state = 'running' AND claimed_by = ? AND attempts = ? \
                   AND lease_expires_at > ?",
        )
        .bind((now + lease).to_string())
        .bind(self.id.raw())
        .bind(self.claimed_by.as_deref())
        .bind(self.attempts)
        .bind(now.to_string())
        .execute(&database.pool)
        .await?;

        Ok(updated.rows_affected() == 1)
    }

    pub async fn finish(&self, database: &Database) -> Result<bool, DatabaseError> {
        let now = Timestamp::now().to_string();
        let updated = sqlx::query(
            "UPDATE jobs SET state = 'done', claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = NULL, finished_at = ? \
             WHERE id = ? AND state = 'running' AND claimed_by = ? AND attempts = ? \
                   AND lease_expires_at > ?",
        )
        .bind(&now)
        .bind(self.id.raw())
        .bind(self.claimed_by.as_deref())
        .bind(self.attempts)
        .bind(&now)
        .execute(&database.pool)
        .await?;

        Ok(updated.rows_affected() == 1)
    }

    /// A job with a retry time goes back in the queue and keeps its error for the record.
    /// Without one it is finished as failed, so the schedule can queue a fresh job later
    /// instead of retrying this one for ever.
    pub async fn fail(
        &self,
        database: &Database,
        message: &str,
        retry_at: Option<Timestamp>,
    ) -> Result<bool, DatabaseError> {
        let state = match retry_at {
            Some(_) => JobState::Queued,
            None => JobState::Failed,
        };

        let now = Timestamp::now().to_string();
        let updated = sqlx::query(
            "UPDATE jobs SET state = ?, claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = ?, available_at = COALESCE(?, available_at), finished_at = ? \
             WHERE id = ? AND state = 'running' AND claimed_by = ? AND attempts = ? \
                   AND lease_expires_at > ?",
        )
        .bind(state.stored())
        .bind(message)
        .bind(retry_at.map(|at| at.to_string()))
        .bind(retry_at.is_none().then_some(now.as_str()))
        .bind(self.id.raw())
        .bind(self.claimed_by.as_deref())
        .bind(self.attempts)
        .bind(&now)
        .execute(&database.pool)
        .await?;

        Ok(updated.rows_affected() == 1)
    }

    /// Waiting for a forge's request budget is not a failed attempt: the work was never
    /// tried. The job keeps its attempt count and waits until the forge says it may ask.
    pub async fn defer(
        &self,
        database: &Database,
        until: Timestamp,
        reason: &str,
    ) -> Result<bool, DatabaseError> {
        let now = Timestamp::now().to_string();
        let updated = sqlx::query(
            "UPDATE jobs SET state = 'queued', claimed_by = NULL, lease_expires_at = NULL, \
                    last_error = ?, available_at = ?, attempts = MAX(attempts - 1, 0) \
             WHERE id = ? AND state = 'running' AND claimed_by = ? AND attempts = ? \
                   AND lease_expires_at > ?",
        )
        .bind(reason)
        .bind(until.to_string())
        .bind(self.id.raw())
        .bind(self.claimed_by.as_deref())
        .bind(self.attempts)
        .bind(&now)
        .execute(&database.pool)
        .await?;

        Ok(updated.rows_affected() == 1)
    }
}

impl DecodeRow for Job {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        Ok(Job {
            id: Id::from_raw(row.try_get("id")?),
            project_id: Id::from_raw(row.try_get("project_id")?),
            kind: JobKind::read(row, "kind")?,
            state: JobState::read(row, "state")?,
            attempts: row.try_get("attempts")?,
            last_error: row.try_get("last_error")?,
            claimed_by: row.try_get("claimed_by")?,
            available_at: Timestamp::read(row, "available_at")?,
            created_at: Timestamp::read(row, "created_at")?,
            finished_at: row
                .try_get::<Option<String>, _>("finished_at")?
                .as_deref()
                .map(Timestamp::from_stored)
                .transpose()?,
        })
    }
}

impl StoredAs for JobKind {
    fn stored(&self) -> &'static str {
        match self {
            JobKind::Discover => "discover",
            JobKind::PackageFacts => "package-facts",
        }
    }
}

impl FromStored for JobKind {
    const FIELD: &'static str = "job_kind";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "discover" => Some(JobKind::Discover),
            "package-facts" => Some(JobKind::PackageFacts),
            _ => None,
        }
    }
}

impl StoredAs for JobState {
    fn stored(&self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
        }
    }
}

impl FromStored for JobState {
    const FIELD: &'static str = "job_state";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(JobState::Queued),
            "running" => Some(JobState::Running),
            "done" => Some(JobState::Done),
            "failed" => Some(JobState::Failed),
            _ => None,
        }
    }
}

/// Runs the schedule and the queue until the process ends. Polling and a webhook would
/// enqueue the same job, so this is the only place analysis starts on its own.
pub async fn serve(state: Arc<AppState>, worker: String) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut cleanup = tokio::time::interval(AUTH_CLEANUP);
    cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = tick(&state, &worker).await {
                    tracing::error!(%error, "queue tick failed");
                }
            }
            () = state.queue_wake.notified() => {
                if let Err(error) = tick(&state, &worker).await {
                    tracing::error!(%error, "queue tick failed");
                }
            }
            _ = cleanup.tick() => match delete_expired_auth_records(&state.database).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(deleted, "removed expired authentication records"),
                Err(error) => tracing::error!(%error, "authentication cleanup failed"),
            },
        }
    }
}

async fn delete_expired_auth_records(database: &Database) -> Result<u64, DatabaseError> {
    let now = Timestamp::now();
    let attempts = attempt::delete_expired(database, now).await?;
    let sessions = session::delete_expired(database, now).await?;
    let tokens = ApiToken::delete_expired(database, now).await?;

    Ok(attempts + sessions + tokens)
}

async fn tick(state: &AppState, worker: &str) -> Result<(), DatabaseError> {
    let reclaimed = Job::reclaim_expired_leases(&state.database).await?;
    if reclaimed > 0 {
        tracing::warn!(reclaimed, "returned jobs whose lease expired");
    }

    let now = Timestamp::now();
    for project_id in Project::due(&state.database, now).await? {
        match Job::enqueue(&state.database, project_id, JobKind::Discover).await? {
            Some(job_id) => tracing::info!(%job_id, %project_id, "queued discovery"),
            None => tracing::debug!(%project_id, "already in the queue"),
        }

        // Marked whether or not a job was inserted: a project already in the queue is
        // covered, and re-reading it every tick would keep it at the head for ever.
        Project::mark_enqueued(&state.database, project_id, now).await?;
    }

    // One job at a time, because two runs of one project would fetch the same mirror
    while let Some(job) = Job::claim(&state.database, worker, LEASE).await? {
        let span = tracing::info_span!(
            "job",
            trace = %crate::trace::new_id(),
            job = %job.id,
            project = %job.project_id,
        );
        run(state, job).instrument(span).await?;
    }

    Ok(())
}

async fn run(state: &AppState, job: Job) -> Result<(), DatabaseError> {
    let started = Timestamp::now();

    match job.kind {
        JobKind::Discover => discover(state, &job, started).await?,
        JobKind::PackageFacts => match registry::fill(state, job.project_id).await {
            Ok(registry::Fill::Failed { reads, retry_at }) => {
                // A registry that is down or refusing is a failed attempt, and the snapshot
                // it leaves unaudited waits on the retry, so the retry must happen.
                let message = format!("{reads} registry reads failed");
                let retry_at = (job.attempts < MAX_ATTEMPTS).then_some(retry_at);

                job.fail(&state.database, &message, retry_at).await?;
                tracing::warn!(
                    attempts = job.attempts,
                    retrying = retry_at.is_some(),
                    "package facts incomplete: {message}"
                );
            }
            Ok(filled) => {
                job.finish(&state.database).await?;
                tracing::info!(
                    seconds = started.duration_until(Timestamp::now()).as_secs(),
                    "package facts filled"
                );

                // One job reads a bounded number of coordinates, and an analysis finishing
                // while this job ran could not queue another, so what is left asks for one.
                if matches!(filled, registry::Fill::Unfinished) {
                    Job::enqueue(&state.database, job.project_id, JobKind::PackageFacts).await?;
                }
            }
            Err(error) => {
                let message = error.to_string();
                let retry_at = (job.attempts < MAX_ATTEMPTS)
                    .then(|| Timestamp::now() + Duration::from_secs(backoff_seconds(job.attempts)));

                job.fail(&state.database, &message, retry_at).await?;
                tracing::warn!(
                    attempts = job.attempts,
                    retrying = retry_at.is_some(),
                    "package facts failed: {message}"
                );
            }
        },
    }

    Ok(())
}

async fn discover(state: &AppState, job: &Job, started: Timestamp) -> Result<(), DatabaseError> {
    let mut renewal = tokio::time::interval(LEASE / 3);
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let discovery = discovery::run(state, job.project_id);
    tokio::pin!(discovery);

    let result = loop {
        tokio::select! {
            result = &mut discovery => break result,
            _ = renewal.tick() => {
                if !job.renew(&state.database, LEASE).await? {
                    tracing::warn!("discovery stopped after losing its lease");
                    return Ok(());
                }
            }
        }
    };

    match result {
        Ok(discovery) => {
            if job.finish(&state.database).await? {
                tracing::info!(
                    changes = discovery.changes.len(),
                    seconds = started.duration_until(Timestamp::now()).as_secs(),
                    "discovery finished"
                );
            } else {
                tracing::warn!("discovery completed after losing its lease");
            }
        }
        Err(error) => match error.rate_limited() {
            Some((host, reset)) => {
                let message = format!("{host} has no request budget left until {reset}");
                if job.defer(&state.database, reset, &message).await? {
                    tracing::warn!(
                        %host,
                        %reset,
                        "discovery is waiting for the forge request budget"
                    );
                } else {
                    tracing::warn!("discovery lost its lease before deferral");
                }
            }
            None => {
                let message = error.to_string();
                let retry_at = (job.attempts < MAX_ATTEMPTS)
                    .then(|| Timestamp::now() + Duration::from_secs(backoff_seconds(job.attempts)));

                if job.fail(&state.database, &message, retry_at).await? {
                    tracing::warn!(
                        attempts = job.attempts,
                        retrying = retry_at.is_some(),
                        "discovery failed: {message}"
                    );
                } else {
                    tracing::warn!("discovery lost its lease before failure was recorded");
                }
            }
        },
    }

    Ok(())
}

fn backoff_seconds(attempts: i64) -> u64 {
    (RETRY_BACKOFF_SECONDS * attempts.max(1)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn queued_job() -> Database {
        let database = Database::open("sqlite::memory:", 0).await.expect("opens");
        let project_id: Id<Project> = database.ids.next();
        sqlx::query(
            "INSERT INTO projects (id, name, remote_url, forge_kind, uses_default_analyzers) \
             VALUES (?, 'lease-test', 'https://github.com/owner/repo', 'auto', 1)",
        )
        .bind(project_id.raw())
        .execute(&database.pool)
        .await
        .expect("inserts project");
        Job::enqueue(&database, project_id, JobKind::Discover)
            .await
            .expect("enqueues");
        database
    }

    async fn expire(database: &Database, job: &Job) {
        sqlx::query("UPDATE jobs SET lease_expires_at = ? WHERE id = ?")
            .bind((Timestamp::now() - Duration::from_secs(1)).to_string())
            .bind(job.id.raw())
            .execute(&database.pool)
            .await
            .expect("expires lease");
    }

    #[tokio::test]
    async fn expired_claim_cannot_change_reclaimed_job() {
        let database = queued_job().await;
        let first = Job::claim(&database, "worker-a", LEASE)
            .await
            .expect("claims")
            .expect("job");
        expire(&database, &first).await;
        assert_eq!(
            Job::reclaim_expired_leases(&database)
                .await
                .expect("reclaims"),
            1
        );
        let second = Job::claim(&database, "worker-b", LEASE)
            .await
            .expect("claims again")
            .expect("job");
        assert!(!first.renew(&database, LEASE).await.expect("checks renewal"));
        assert!(!first.finish(&database).await.expect("checks finish"));
        assert!(
            !first
                .fail(&database, "stale failure", None)
                .await
                .expect("checks failure")
        );
        assert!(
            !first
                .defer(&database, Timestamp::now(), "stale deferral")
                .await
                .expect("checks deferral")
        );
        let running = Job::recent(&database, Some(first.project_id), 1)
            .await
            .expect("reads current claim");
        assert_eq!(running[0].state, JobState::Running);
        assert_eq!(running[0].attempts, second.attempts);
        assert_eq!(running[0].last_error.as_deref(), Some("lease expired"));
        expire(&database, &second).await;
        assert_eq!(
            Job::reclaim_expired_leases(&database)
                .await
                .expect("reclaims again"),
            1
        );
        let third = Job::claim(&database, "worker-a", LEASE)
            .await
            .expect("claims third time")
            .expect("job");
        assert!(!first.finish(&database).await.expect("checks oldest claim"));
        assert!(
            !second
                .fail(&database, "stale failure", None)
                .await
                .expect("checks second claim")
        );
        assert!(
            third
                .finish(&database)
                .await
                .expect("finishes current claim")
        );
    }

    #[tokio::test]
    async fn deferred_claim_cannot_change_next_claim_with_same_attempt_number() {
        let database = queued_job().await;
        let first = Job::claim(&database, "worker", LEASE)
            .await
            .expect("claims")
            .expect("job");
        assert!(
            first
                .defer(&database, Timestamp::now(), "budget exhausted")
                .await
                .expect("defers")
        );
        let second = Job::claim(&database, "worker", LEASE)
            .await
            .expect("claims again")
            .expect("job");
        assert_eq!(first.attempts, second.attempts);
        assert!(!first.finish(&database).await.expect("checks stale finish"));
        assert!(
            !first
                .fail(&database, "stale", None)
                .await
                .expect("checks stale failure")
        );
        assert!(
            !first
                .defer(&database, Timestamp::now(), "stale")
                .await
                .expect("checks stale deferral")
        );
        assert!(
            second
                .renew(&database, LEASE)
                .await
                .expect("renews current claim")
        );
        assert!(
            second
                .finish(&database)
                .await
                .expect("finishes current claim")
        );
    }

    #[tokio::test]
    async fn renewal_keeps_honest_work_claimed() {
        let database = queued_job().await;
        let job = Job::claim(&database, "worker", Duration::from_secs(1))
            .await
            .expect("claims")
            .expect("job");
        assert!(job.renew(&database, LEASE).await.expect("renews"));
        assert_eq!(
            Job::reclaim_expired_leases(&database)
                .await
                .expect("checks expiry"),
            0
        );
        assert!(job.finish(&database).await.expect("finishes"));
        assert!(!job.renew(&database, LEASE).await.expect("renewal stopped"));
    }
}
