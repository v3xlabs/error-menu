use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};

use crate::app::AppState;
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::prelude::*;
use crate::registry::{self, Coordinate};
use crate::user::token::ApiToken;
use crate::user::{attempt, session};
use crate::worker::discovery;
use crate::worker::resource::{POOL, Resource};
use tracing::Instrument;

/// A stopped process loses its claim after this long; active discovery renews it.
pub const LEASE: Duration = Duration::from_secs(900);

/// How often the schedule looks for projects that are due and leases that expired, and how
/// long an idle worker waits before it looks at the queue again without being woken.
const TICK: Duration = Duration::from_secs(15);

/// How often the schedule queues registry reads that nothing else queued.
const SWEEP: Duration = Duration::from_secs(600);

/// A job that keeps failing stops asking. Three attempts with a widening wait is enough to
/// ride out a forge that is rate limiting or briefly down.
const MAX_ATTEMPTS: i64 = 3;
const RETRY_BACKOFF_SECONDS: i64 = 120;

/// Expired sessions, tokens, login attempts and resource blocks are already invisible to
/// what reads them, so removing them is housekeeping and belongs on its own slow schedule.
const CLEANUP: Duration = Duration::from_secs(3600);

/// Every package version is a job, so finished jobs are kept long enough to read what
/// happened and no longer.
const JOB_HISTORY: Duration = Duration::from_secs(7 * 24 * 3600);

/// What a job does, and to what. The kind and the subject together are the job's identity:
/// asking twice for the same work while it waits is one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    Discover { project_id: Id<Project> },
    PackageFacts(Coordinate),
    PackageAudit { project_id: Id<Project> },
}

impl Work {
    pub fn kind(&self) -> &'static str {
        match self {
            Work::Discover { .. } => "discover",
            Work::PackageFacts(_) => "package-facts",
            Work::PackageAudit { .. } => "package-audit",
        }
    }

    pub fn subject(&self) -> String {
        match self {
            Work::Discover { project_id } | Work::PackageAudit { project_id } => {
                project_id.raw().to_string()
            }
            Work::PackageFacts(coordinate) => coordinate.subject(),
        }
    }

    pub fn project_id(&self) -> Option<Id<Project>> {
        match self {
            Work::Discover { project_id } | Work::PackageAudit { project_id } => Some(*project_id),
            Work::PackageFacts(_) => None,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        match self {
            Work::Discover { project_id } => {
                vec![Resource::Mirror(*project_id), Resource::Discovery]
            }
            Work::PackageFacts(coordinate) => match coordinate.ecosystem {
                Ecosystem::Cargo => vec![Resource::CratesIo, Resource::Osv],
                Ecosystem::Npm => vec![Resource::Npm],
                Ecosystem::Nix => Vec::new(),
            },
            Work::PackageAudit { .. } => vec![Resource::Audit],
        }
    }

    /// Whether a running job already answers a new request. Discovery reads the forge when it
    /// starts and a fetch reads the registry, so a second one would ask the same question.
    /// An audit may have checked readiness before the answer that makes a snapshot ready
    /// arrived, so a request during one must run again after it.
    fn covered_while_running(&self) -> bool {
        !matches!(self, Work::PackageAudit { .. })
    }

    fn decode(
        kind: &str,
        subject: &str,
        project_id: Option<Id<Project>>,
    ) -> Result<Work, DatabaseError> {
        let unreadable = |field: &'static str, value: &str| DatabaseError::Unreadable {
            field,
            value: value.to_owned(),
        };
        let project = || project_id.ok_or_else(|| unreadable("job project", subject));

        match kind {
            "discover" => Ok(Work::Discover {
                project_id: project()?,
            }),
            "package-facts" => Ok(Work::PackageFacts(Coordinate::from_subject(subject)?)),
            "package-audit" => Ok(Work::PackageAudit {
                project_id: project()?,
            }),
            _ => Err(unreadable("job_kind", kind)),
        }
    }
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
    pub work: Work,
    pub state: JobState,
    pub attempts: i64,
    pub last_error: Option<String>,
    claimed_by: Option<String>,
    pub available_at: Timestamp,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// Work a reader is waiting on goes before work that only refills what is missing, so a
/// backlog of old versions never delays the answer to a scan that just finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Backfill,
    Requested,
}

impl Priority {
    fn value(self) -> i64 {
        match self {
            Priority::Backfill => -1,
            Priority::Requested => 0,
        }
    }
}

impl Job {
    /// Queues work unless the same work is already waiting, or running where a run already
    /// covers it.
    pub async fn enqueue(
        database: &Database,
        work: &Work,
    ) -> Result<Option<Id<Job>>, DatabaseError> {
        let mut transaction = database.write().await?;
        let id = insert(
            database,
            &mut transaction,
            work,
            Priority::Requested,
            &Timestamp::now().to_string(),
        )
        .await?;
        transaction.commit().await?;

        Ok(id)
    }

    /// Queues every piece of work in one transaction, so an analysis that names a thousand
    /// versions takes the writer once. Answers how many were new.
    pub async fn enqueue_all(
        database: &Database,
        work: &[Work],
        priority: Priority,
    ) -> Result<usize, DatabaseError> {
        if work.is_empty() {
            return Ok(0);
        }

        let now = Timestamp::now().to_string();
        let mut transaction = database.write().await?;
        let mut inserted = 0;
        for work in work {
            if insert(database, &mut transaction, work, priority, &now)
                .await?
                .is_some()
            {
                inserted += 1;
            }
        }
        transaction.commit().await?;

        Ok(inserted)
    }

    /// Takes the oldest ready job of the highest priority whose resources all have room and
    /// none is blocked, and holds it for the lease. A job that cannot run yet is passed over
    /// rather than waited on, so one busy registry never holds up anything else. The claim
    /// holds the writer, so two workers cannot fill the same last slot.
    pub async fn claim(
        database: &Database,
        worker: &str,
        lease: Duration,
    ) -> Result<Option<Job>, DatabaseError> {
        let now = Timestamp::now();
        let claim = format!("{worker}:{}", crate::trace::new_id());
        let mut transaction = database.write().await?;
        let row = sqlx::query(
            "WITH busy AS ( \
                 SELECT held.resource FROM job_resources held \
                 JOIN jobs holder ON holder.id = held.job_id \
                 WHERE holder.state = 'running' \
                 GROUP BY held.resource HAVING COUNT(*) >= MIN(held.capacity) \
             ), blocked AS ( \
                 SELECT resource FROM resource_blocks WHERE blocked_until > ?1 \
             ) \
             UPDATE jobs SET state = 'running', claimed_by = ?2, lease_expires_at = ?3, \
                    attempts = attempts + 1 \
             WHERE id = ( \
                 SELECT j.id FROM jobs j \
                 WHERE j.state = 'queued' AND j.available_at <= ?1 \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM job_resources needed \
                       WHERE needed.job_id = j.id \
                         AND (needed.resource IN busy OR needed.resource IN blocked) \
                   ) \
                 ORDER BY j.priority DESC, j.id LIMIT 1 \
             ) \
             RETURNING id, project_id, kind, subject, state, attempts, claimed_by, last_error, \
                       available_at, created_at, finished_at",
        )
        .bind(now.to_string())
        .bind(&claim)
        .bind((now + lease).to_string())
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;

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
            "SELECT id, project_id, kind, subject, state, attempts, claimed_by, last_error, \
                    available_at, created_at, finished_at \
             FROM jobs WHERE (?1 IS NULL OR project_id = ?1) ORDER BY id DESC LIMIT ?2",
        )
        .bind(project_id.map(Id::raw))
        .bind(limit)
        .fetch_all(&database.pool)
        .await?;

        rows.iter().map(Job::decode_row).collect()
    }

    /// Forgets jobs that ended before `before`. A waiting or running job is never forgotten.
    pub async fn prune(database: &Database, before: Timestamp) -> Result<u64, DatabaseError> {
        let pruned =
            sqlx::query("DELETE FROM jobs WHERE state IN ('done', 'failed') AND finished_at < ?")
                .bind(before.to_string())
                .execute(&database.pool)
                .await?;

        Ok(pruned.rows_affected())
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

    /// Waiting for a budget is not a failed attempt: the work was never tried. The job keeps
    /// its attempt count and waits until it may ask.
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

/// Work already waiting is not queued twice, but it takes the higher of the two priorities:
/// a version the sweep queued moves up when a scan asks for it.
async fn insert(
    database: &Database,
    connection: &mut SqliteConnection,
    work: &Work,
    priority: Priority,
    now: &str,
) -> Result<Option<Id<Job>>, DatabaseError> {
    let id = database.ids.next::<Job>();
    let subject = work.subject();
    let query = if work.covered_while_running() {
        "INSERT INTO jobs (id, project_id, kind, subject, priority, state, available_at, created_at) \
         SELECT ?, ?, ?, ?, ?, 'queued', ?, ? \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM jobs WHERE kind = ? AND subject = ? AND state IN ('queued', 'running') \
         )"
    } else {
        "INSERT INTO jobs (id, project_id, kind, subject, priority, state, available_at, created_at) \
         SELECT ?, ?, ?, ?, ?, 'queued', ?, ? \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM jobs WHERE kind = ? AND subject = ? AND state = 'queued' \
         )"
    };
    let inserted = sqlx::query(query)
        .bind(id.raw())
        .bind(work.project_id().map(Id::raw))
        .bind(work.kind())
        .bind(&subject)
        .bind(priority.value())
        .bind(now)
        .bind(now)
        .bind(work.kind())
        .bind(&subject)
        .execute(&mut *connection)
        .await?;
    if inserted.rows_affected() == 0 {
        sqlx::query(
            "UPDATE jobs SET priority = MAX(priority, ?) \
             WHERE kind = ? AND subject = ? AND state = 'queued'",
        )
        .bind(priority.value())
        .bind(work.kind())
        .bind(&subject)
        .execute(&mut *connection)
        .await?;
        return Ok(None);
    }

    for resource in work.resources() {
        sqlx::query("INSERT INTO job_resources (job_id, resource, capacity) VALUES (?, ?, ?)")
            .bind(id.raw())
            .bind(resource.key())
            .bind(resource.capacity() as i64)
            .execute(&mut *connection)
            .await?;
    }

    Ok(Some(id))
}

impl DecodeRow for Job {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        let project_id = row
            .try_get::<Option<i64>, _>("project_id")?
            .map(Id::from_raw);

        Ok(Job {
            id: Id::from_raw(row.try_get("id")?),
            work: Work::decode(row.try_get("kind")?, row.try_get("subject")?, project_id)?,
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

/// Runs the schedule and a pool of workers until the process ends. Polling and a webhook
/// would queue the same job, so this is the only place analysis starts on its own.
pub async fn serve(state: Arc<AppState>, worker: String) {
    for slot in 0..POOL {
        tokio::spawn(work(Arc::clone(&state), format!("{worker}-{slot}")));
    }

    schedule(state).await;
}

/// Keeps the queue fed. It never runs a job itself, so a long job cannot delay a due
/// project being queued.
async fn schedule(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sweep = tokio::time::interval(SWEEP);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut cleanup = tokio::time::interval(CLEANUP);
    cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = tick(&state).await {
                    tracing::error!(%error, "queue tick failed");
                }
            }
            _ = sweep.tick() => match registry::sweep(&state.database).await {
                Ok(0) => {}
                Ok(queued) => {
                    tracing::info!(queued, "queued registry reads nothing else asked for");
                    state.queue_wake.notify_waiters();
                }
                Err(error) => tracing::error!(%error, "registry sweep failed"),
            },
            _ = cleanup.tick() => match clean_up(&state.database).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(deleted, "removed expired records"),
                Err(error) => tracing::error!(%error, "cleanup failed"),
            },
        }
    }
}

async fn clean_up(database: &Database) -> Result<u64, DatabaseError> {
    let now = Timestamp::now();
    let attempts = attempt::delete_expired(database, now).await?;
    let sessions = session::delete_expired(database, now).await?;
    let tokens = ApiToken::delete_expired(database, now).await?;
    let blocks = Resource::clear_expired_blocks(database).await?;
    let jobs = Job::prune(database, now - JOB_HISTORY).await?;

    Ok(attempts + sessions + tokens + blocks + jobs)
}

async fn tick(state: &AppState) -> Result<(), DatabaseError> {
    let reclaimed = Job::reclaim_expired_leases(&state.database).await?;
    if reclaimed > 0 {
        tracing::warn!(reclaimed, "returned jobs whose lease expired");
    }

    let now = Timestamp::now();
    for project_id in Project::due(&state.database, now).await? {
        match Job::enqueue(&state.database, &Work::Discover { project_id }).await? {
            Some(job_id) => tracing::info!(%job_id, %project_id, "queued discovery"),
            None => tracing::debug!(%project_id, "already in the queue"),
        }

        // Marked whether or not a job was inserted: a project already in the queue is
        // covered, and re-reading it every tick would keep it at the head for ever.
        Project::mark_enqueued(&state.database, project_id, now).await?;
    }
    state.queue_wake.notify_waiters();

    Ok(())
}

/// One worker: claims whatever can run, runs it, and sleeps until woken or the next tick
/// when nothing can.
///
/// A worker that claims a job wakes one other, and so does a worker that finishes one. The
/// chain stops at the first worker that finds nothing, so an event costs one claim per job it
/// makes runnable rather than one per idle worker: every claim takes the writer.
async fn work(state: Arc<AppState>, worker: String) {
    loop {
        let claimed = {
            // Registered before the claim, so work queued between an empty claim and the
            // wait still wakes this worker. Dropped before this worker wakes anyone, so it
            // never wakes itself.
            let mut woken = std::pin::pin!(state.queue_wake.notified());
            woken.as_mut().enable();

            match Job::claim(&state.database, &worker, LEASE).await {
                Ok(claimed) => {
                    if claimed.is_none() {
                        tokio::select! {
                            () = &mut woken => {}
                            () = tokio::time::sleep(TICK) => {}
                        }
                    }
                    claimed
                }
                Err(error) => {
                    tracing::error!(%error, "claiming a job failed");
                    tokio::time::sleep(TICK).await;
                    None
                }
            }
        };

        let Some(job) = claimed else { continue };
        state.queue_wake.notify_one();
        let span = tracing::info_span!(
            "job",
            trace = %crate::trace::new_id(),
            job = %job.id,
            kind = job.work.kind(),
            subject = %job.work.subject(),
        );
        if let Err(error) = run(&state, job).instrument(span).await {
            tracing::error!(%error, "job failed to record its outcome");
        }
        state.queue_wake.notify_one();
    }
}

async fn run(state: &AppState, job: Job) -> Result<(), DatabaseError> {
    let started = Timestamp::now();

    match &job.work {
        Work::Discover { project_id } => discover(state, &job, *project_id, started).await,
        Work::PackageFacts(coordinate) => package_facts(state, &job, coordinate).await,
        Work::PackageAudit { project_id } => {
            match registry::audit(&state.database, *project_id).await {
                Ok(()) => {
                    job.finish(&state.database).await?;
                }
                Err(error) => retry_or_fail(state, &job, &error.to_string()).await?,
            }
            Ok(())
        }
    }
}

async fn package_facts(
    state: &AppState,
    job: &Job,
    coordinate: &Coordinate,
) -> Result<(), DatabaseError> {
    match registry::fetch(state, coordinate).await {
        Ok(registry::Fetch::Answered) => {
            if job.finish(&state.database).await? {
                registry::queue_audits(&state.database, coordinate).await?;
            }
        }
        Ok(registry::Fetch::Failed { retry_at }) => {
            // The failure is cached until `retry_at`, and the snapshots that name this
            // version wait on an answer, so the retry must happen then.
            let retry_at = (job.attempts < MAX_ATTEMPTS).then_some(retry_at);
            job.fail(&state.database, "registry read failed", retry_at)
                .await?;
        }
        Err(registry::RegistryError::RateLimited { resource, until }) => {
            let reason = format!("{} asked us to wait until {until}", resource.key());
            resource.block(&state.database, until, &reason).await?;
            job.defer(&state.database, until, &reason).await?;
            tracing::warn!(resource = resource.key(), %until, "registry is rate limiting");
        }
        Err(error) => retry_or_fail(state, job, &error.to_string()).await?,
    }

    Ok(())
}

async fn retry_or_fail(state: &AppState, job: &Job, message: &str) -> Result<(), DatabaseError> {
    let retry_at = (job.attempts < MAX_ATTEMPTS)
        .then(|| Timestamp::now() + Duration::from_secs(backoff_seconds(job.attempts)));

    job.fail(&state.database, message, retry_at).await?;
    tracing::warn!(
        attempts = job.attempts,
        retrying = retry_at.is_some(),
        "{} failed: {message}",
        job.work.kind()
    );

    Ok(())
}

async fn discover(
    state: &AppState,
    job: &Job,
    project_id: Id<Project>,
    started: Timestamp,
) -> Result<(), DatabaseError> {
    let mut renewal = tokio::time::interval(LEASE / 3);
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let discovery = discovery::run(state, project_id);
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
            // Budgets belong to a credential, and projects on one forge can hold different
            // ones, so the wait is this job's rather than a block on the whole forge.
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

    async fn database() -> Database {
        Database::open("sqlite::memory:", 0).await.expect("opens")
    }

    async fn project(database: &Database) -> Id<Project> {
        let project_id: Id<Project> = database.ids.next();
        sqlx::query(
            "INSERT INTO projects (id, name, remote_url, forge_kind, uses_default_analyzers) \
             VALUES (?, 'lease-test', 'https://github.com/owner/repo', 'auto', 1)",
        )
        .bind(project_id.raw())
        .execute(&database.pool)
        .await
        .expect("inserts project");

        project_id
    }

    async fn queued_job() -> Database {
        let database = database().await;
        let project_id = project(&database).await;
        Job::enqueue(&database, &Work::Discover { project_id })
            .await
            .expect("enqueues");
        database
    }

    fn package(ecosystem: Ecosystem, name: &str) -> Work {
        Work::PackageFacts(Coordinate {
            ecosystem,
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
        })
    }

    async fn claim(database: &Database) -> Option<Work> {
        Job::claim(database, "worker", LEASE)
            .await
            .expect("claims")
            .map(|job| job.work)
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
    async fn a_full_resource_is_passed_over_for_work_that_has_room() {
        let database = database().await;
        Job::enqueue_all(
            &database,
            &[
                package(Ecosystem::Cargo, "serde"),
                package(Ecosystem::Cargo, "anyhow"),
                package(Ecosystem::Npm, "@scope/left-pad"),
            ],
            Priority::Requested,
        )
        .await
        .expect("enqueues");

        let serde = Job::claim(&database, "worker", LEASE)
            .await
            .expect("claims")
            .expect("serde runs first");
        assert_eq!(serde.work, package(Ecosystem::Cargo, "serde"));
        assert_eq!(
            claim(&database).await,
            Some(package(Ecosystem::Npm, "@scope/left-pad")),
            "the npm read waited behind a crates.io read"
        );
        assert_eq!(
            claim(&database).await,
            None,
            "two crates.io reads ran at once"
        );

        assert!(serde.finish(&database).await.expect("finishes"));
        assert_eq!(
            claim(&database).await,
            Some(package(Ecosystem::Cargo, "anyhow"))
        );
    }

    #[tokio::test]
    async fn a_scan_moves_its_versions_ahead_of_the_backfill() {
        let database = database().await;
        Job::enqueue_all(
            &database,
            &[
                package(Ecosystem::Cargo, "serde"),
                package(Ecosystem::Cargo, "anyhow"),
            ],
            Priority::Backfill,
        )
        .await
        .expect("backfills");
        Job::enqueue_all(
            &database,
            &[package(Ecosystem::Cargo, "anyhow")],
            Priority::Requested,
        )
        .await
        .expect("requests");

        assert_eq!(
            claim(&database).await,
            Some(package(Ecosystem::Cargo, "anyhow")),
            "a scan's version waited behind the backfill"
        );
    }

    #[tokio::test]
    async fn a_blocked_resource_holds_only_the_work_that_needs_it() {
        let database = database().await;
        let project_id = project(&database).await;
        Job::enqueue_all(
            &database,
            &[
                package(Ecosystem::Npm, "undici"),
                Work::Discover { project_id },
            ],
            Priority::Requested,
        )
        .await
        .expect("enqueues");
        Resource::Npm
            .block(&database, Timestamp::now() + Duration::from_secs(60), "429")
            .await
            .expect("blocks");

        assert_eq!(claim(&database).await, Some(Work::Discover { project_id }));
        assert_eq!(claim(&database).await, None, "npm ran while blocked");

        Resource::Npm
            .block(&database, Timestamp::now() - Duration::from_secs(1), "429")
            .await
            .expect("keeps the longer block");
        assert_eq!(
            claim(&database).await,
            None,
            "a shorter block cut a longer one"
        );
    }

    #[tokio::test]
    async fn one_version_is_one_job_whoever_asks() {
        let database = database().await;
        let undici = package(Ecosystem::Npm, "undici");

        assert!(
            Job::enqueue(&database, &undici)
                .await
                .expect("enqueues")
                .is_some()
        );
        assert!(
            Job::enqueue(&database, &undici)
                .await
                .expect("enqueues")
                .is_none(),
            "a second project's request queued the same read again"
        );
    }

    #[tokio::test]
    async fn an_audit_asked_for_while_one_runs_runs_again() {
        let database = database().await;
        let project_id = project(&database).await;
        let audit = Work::PackageAudit { project_id };
        Job::enqueue(&database, &audit).await.expect("enqueues");
        let running = Job::claim(&database, "worker", LEASE)
            .await
            .expect("claims")
            .expect("audit");

        assert!(
            Job::enqueue(&database, &audit)
                .await
                .expect("enqueues")
                .is_some(),
            "an answer that arrived during the audit was never audited"
        );
        assert_eq!(claim(&database).await, None, "two audits ran at once");
        assert!(running.finish(&database).await.expect("finishes"));
        assert_eq!(claim(&database).await, Some(audit));
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
        let running = Job::recent(&database, first.work.project_id(), 1)
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
