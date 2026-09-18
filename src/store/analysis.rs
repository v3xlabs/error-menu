use jiff::Timestamp;
use sqlx::{Row, SqliteConnection};

use crate::analysis::ci_checks::{CheckConclusion, CheckRun, CheckStatus};
use crate::analysis::{Run, RunStatus};
use crate::confidence::Confidence;
use crate::finding::fingerprint::Fingerprint;
use crate::finding::issue::{Issue, Triage};
use crate::finding::{
    Attribution, Ecosystem, Finding, LineSpan, Location, NewFinding, Severity, VersionMovement,
};
use crate::id::{Id, IdGenerator};
use crate::signal::{NewSignal, Score, Signal, SignalKey, SignalValue};
use crate::vcs::RepoPath;
use crate::watch::{Project, Snapshot, Subject};

use super::snapshots::{decode_snapshot, decode_subject};
use super::{AnalysisRecord, AnalysisRunRecord, RunRecord, Store, StoreError};

impl Store {
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

    pub(crate) async fn check_runs_for_snapshot(
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
