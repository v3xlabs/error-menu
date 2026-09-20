use sqlx::Row;

use crate::database::codec::{FromStored, StoredAs};
use crate::prelude::*;

pub const ANALYZER: &str = "ci-check-runs";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Neutral,
    Skipped,
    Cancelled,
    TimedOut,
    ActionRequired,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub url: Option<String>,
    pub log_excerpt_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCheck {
    pub name: String,
    pub url: Option<String>,
    pub log_excerpt_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub signal: Option<NewSignal>,
    pub failed_checks: Vec<FailedCheck>,
}

pub fn classify(checks: &[CheckRun]) -> Classification {
    let failed_checks = checks
        .iter()
        .filter(|check| {
            check.status == CheckStatus::Completed
                && check.conclusion == Some(CheckConclusion::Failure)
        })
        .map(|check| FailedCheck {
            name: check.name.clone(),
            url: check.url.clone(),
            log_excerpt_ref: check.log_excerpt_ref.clone(),
        })
        .collect::<Vec<_>>();
    let signal = (!failed_checks.is_empty()).then(|| NewSignal {
        key: SignalKey::TestsFailing,
        value: SignalValue::Count(failed_checks.len() as u64),
        confidence: Confidence::new(1.0).expect("confidence is in range"),
        reason: format!("{} completed CI checks failed.", failed_checks.len()),
    });
    Classification {
        signal,
        failed_checks,
    }
}

impl CheckRun {
    pub async fn replace(
        database: &Database,
        snapshot_id: Id<Snapshot>,
        check_runs: &[CheckRun],
    ) -> Result<(), DatabaseError> {
        let mut transaction = database.pool.begin().await?;
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
            .bind(check_run.status.stored())
            .bind(check_run.conclusion.map(|conclusion| conclusion.stored()))
            .bind(&check_run.url)
            .bind(&check_run.log_excerpt_ref)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    pub async fn for_snapshot(
        database: &Database,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<CheckRun>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT name, status, conclusion, url, log_excerpt_ref \
             FROM check_runs WHERE snapshot_id = \
                 (SELECT COALESCE(analysis_snapshot_id, id) FROM snapshots WHERE id = ?) ORDER BY id",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&database.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(CheckRun {
                    name: row.try_get("name")?,
                    status: CheckStatus::read(&row, "status")?,
                    conclusion: row
                        .try_get::<Option<String>, _>("conclusion")?
                        .map(|value| CheckConclusion::from_stored(&value))
                        .transpose()?,
                    url: row.try_get("url")?,
                    log_excerpt_ref: row.try_get("log_excerpt_ref")?,
                })
            })
            .collect()
    }
}

impl StoredAs for CheckStatus {
    fn stored(&self) -> &'static str {
        match self {
            CheckStatus::Queued => "queued",
            CheckStatus::InProgress => "in_progress",
            CheckStatus::Completed => "completed",
        }
    }
}

impl FromStored for CheckStatus {
    const FIELD: &'static str = "check_run_status";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(CheckStatus::Queued),
            "in_progress" => Some(CheckStatus::InProgress),
            "completed" => Some(CheckStatus::Completed),
            _ => None,
        }
    }
}

impl StoredAs for CheckConclusion {
    fn stored(&self) -> &'static str {
        match self {
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
}

impl FromStored for CheckConclusion {
    const FIELD: &'static str = "check_run_conclusion";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "success" => Some(CheckConclusion::Success),
            "failure" => Some(CheckConclusion::Failure),
            "neutral" => Some(CheckConclusion::Neutral),
            "skipped" => Some(CheckConclusion::Skipped),
            "cancelled" => Some(CheckConclusion::Cancelled),
            "timed_out" => Some(CheckConclusion::TimedOut),
            "action_required" => Some(CheckConclusion::ActionRequired),
            "stale" => Some(CheckConclusion::Stale),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, status: CheckStatus, conclusion: Option<CheckConclusion>) -> CheckRun {
        CheckRun {
            name: name.to_owned(),
            status,
            conclusion,
            url: Some(format!("https://ci.example.invalid/{name}")),
            log_excerpt_ref: Some(format!("log:{name}")),
        }
    }

    #[test]
    fn emits_signal_and_evidence_only_for_completed_failures() {
        let checks = [
            check(
                "unit",
                CheckStatus::Completed,
                Some(CheckConclusion::Failure),
            ),
            check(
                "integration",
                CheckStatus::Completed,
                Some(CheckConclusion::Failure),
            ),
            check(
                "pending",
                CheckStatus::InProgress,
                Some(CheckConclusion::Failure),
            ),
            check(
                "build",
                CheckStatus::Completed,
                Some(CheckConclusion::Success),
            ),
            check(
                "lint",
                CheckStatus::Completed,
                Some(CheckConclusion::Cancelled),
            ),
        ];
        let result = classify(&checks);
        assert_eq!(
            result.signal.as_ref().map(|signal| signal.value),
            Some(SignalValue::Count(2))
        );
        assert_eq!(result.failed_checks.len(), 2);
        assert_eq!(result.failed_checks[0].name, "unit");
        assert_eq!(
            result.failed_checks[0].url.as_deref(),
            Some("https://ci.example.invalid/unit")
        );
        assert_eq!(
            result.failed_checks[0].log_excerpt_ref.as_deref(),
            Some("log:unit")
        );
    }

    #[test]
    fn missing_conclusions_and_non_failures_are_clear() {
        let checks = [
            check("queued", CheckStatus::Queued, None),
            check("running", CheckStatus::InProgress, None),
            check("unknown", CheckStatus::Completed, None),
            check(
                "neutral",
                CheckStatus::Completed,
                Some(CheckConclusion::Neutral),
            ),
        ];
        let result = classify(&checks);
        assert!(result.signal.is_none());
        assert!(result.failed_checks.is_empty());
    }
}
