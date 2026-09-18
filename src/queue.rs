use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;

use crate::discovery;
use crate::id::Id;
use crate::store::Store;
use crate::watch::Project;

/// How long a claim holds a job before another worker may take it. It has to outlast the
/// slowest honest run: a first clone of a large repository plus a walk of its open changes.
pub const LEASE: Duration = Duration::from_secs(900);

/// How often the app looks for projects that are due and jobs that are ready. The interval
/// bounds how late a due project starts, not how often a project is polled.
const TICK: Duration = Duration::from_secs(15);

/// A job that keeps failing stops asking. Three attempts with a widening wait is enough to
/// ride out a forge that is rate limiting or briefly down.
const MAX_ATTEMPTS: i64 = 3;
const RETRY_BACKOFF_SECONDS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Discover,
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
    pub attempts: i64,
}

#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: Id<Job>,
    pub project_id: Id<Project>,
    pub kind: JobKind,
    pub state: JobState,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub available_at: Timestamp,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// Runs the schedule and the queue until the process ends. Polling and a webhook would
/// enqueue the same job, so this is the only place analysis starts on its own.
pub async fn serve(store: Arc<Store>, mirror_root: PathBuf, worker: String) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        if let Err(error) = tick(&store, &mirror_root, &worker).await {
            tracing::error!(%error, "queue tick failed");
        }
    }
}

async fn tick(store: &Store, mirror_root: &Path, worker: &str) -> Result<(), crate::store::StoreError> {
    let reclaimed = store.reclaim_expired_leases().await?;
    if reclaimed > 0 {
        tracing::warn!(reclaimed, "returned jobs whose lease expired");
    }

    let now = Timestamp::now();
    for project_id in store.projects_due(now).await? {
        match store.enqueue(project_id, JobKind::Discover).await? {
            Some(job_id) => tracing::info!(%job_id, %project_id, "queued discovery"),
            None => tracing::debug!(%project_id, "already in the queue"),
        }

        // Marked whether or not a job was inserted: a project already in the queue is
        // covered, and re-reading it every tick would keep it at the head for ever.
        store.mark_enqueued(project_id, now).await?;
    }

    // One job at a time, because two runs of one project would fetch the same mirror
    while let Some(job) = store.claim(worker, LEASE).await? {
        run(store, mirror_root, job).await?;
    }

    Ok(())
}

async fn run(store: &Store, mirror_root: &Path, job: Job) -> Result<(), crate::store::StoreError> {
    let started = Timestamp::now();

    match job.kind {
        JobKind::Discover => match discovery::run(store, mirror_root, job.project_id).await {
            Ok(discovery) => {
                store.finish(job.id).await?;
                tracing::info!(
                    job = %job.id,
                    project = %job.project_id,
                    changes = discovery.changes.len(),
                    seconds = started.duration_until(Timestamp::now()).as_secs(),
                    "discovery finished"
                );
            }
            Err(error) => {
                let message = error.to_string();
                let retry_at = (job.attempts < MAX_ATTEMPTS)
                    .then(|| Timestamp::now() + Duration::from_secs(backoff_seconds(job.attempts)));

                store.fail(job.id, &message, retry_at).await?;
                tracing::warn!(
                    job = %job.id,
                    project = %job.project_id,
                    attempts = job.attempts,
                    retrying = retry_at.is_some(),
                    "discovery failed: {message}"
                );
            }
        },
    }

    Ok(())
}

fn backoff_seconds(attempts: i64) -> u64 {
    (RETRY_BACKOFF_SECONDS * attempts.max(1)) as u64
}

pub fn encode_kind(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Discover => "discover",
    }
}

pub fn decode_kind(value: &str) -> Option<JobKind> {
    match value {
        "discover" => Some(JobKind::Discover),
        _ => None,
    }
}

pub fn encode_state(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Done => "done",
        JobState::Failed => "failed",
    }
}

pub fn decode_state(value: &str) -> Option<JobState> {
    match value {
        "queued" => Some(JobState::Queued),
        "running" => Some(JobState::Running),
        "done" => Some(JobState::Done),
        "failed" => Some(JobState::Failed),
        _ => None,
    }
}
