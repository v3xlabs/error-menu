use jiff::Timestamp;
use sqlx::Row;

use crate::id::Id;
use crate::queue::{self, Job, JobKind, JobRecord, JobState};
use crate::watch::Project;

use super::{Store, StoreError, decode_timestamp};

impl Store {
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
                Some(value) => decode_timestamp(value)?.duration_until(now).as_secs() >= interval,
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

    pub async fn mark_enqueued(
        &self,
        project_id: Id<Project>,
        at: Timestamp,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE projects SET last_enqueued_at = ? WHERE id = ?")
            .bind(at.to_string())
            .bind(project_id.raw())
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
