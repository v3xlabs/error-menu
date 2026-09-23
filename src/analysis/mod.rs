pub mod ci_checks;
pub mod confidence;
pub mod finding;
pub mod hygiene;
pub mod links;
pub mod lockfile;
pub mod manifest;
pub mod repository_controls;
pub mod runner;
pub mod secret;
pub mod signal;
pub mod workflow;

use std::collections::HashMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::analysis::ci_checks::CheckRun;
use crate::analysis::finding::fingerprint::Fingerprint;
use crate::analysis::finding::issue::{Issue, Triage};
use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::id::IdGenerator;
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: Id<Run>,
    pub snapshot_id: Id<Snapshot>,
    pub analyzer: String,
    pub status: RunStatus,
    pub compared_against: Option<Id<Snapshot>>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// A run carries its own outcome so that "the analyzer failed" can never be read as
/// "the analyzer found nothing".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed { message: String },
    TimedOut,
    Skipped { reason: String },
}

impl RunStatus {
    pub fn is_comparable(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// Everything one analyzer produced against one snapshot. Findings, signals and the run's
/// own status commit together, so a crashed worker leaves no half-recorded run.
pub struct NewRun<'a> {
    pub snapshot_id: Id<Snapshot>,
    pub analyzer: &'a str,
    pub status: RunStatus,
    pub compared_against: Option<Id<Snapshot>>,
    pub findings: &'a [NewFinding],
    pub signals: &'a [NewSignal],
}

pub struct SnapshotAnalysis {
    pub subject: Subject,
    pub snapshot: Snapshot,
    pub check_runs: Vec<CheckRun>,
    pub runs: Vec<CompletedRun>,
}

pub struct CompletedRun {
    pub run: Run,
    pub findings: Vec<Finding>,
    pub signals: Vec<Signal>,
}

impl Run {
    pub async fn record(database: &Database, run: NewRun<'_>) -> Result<Run, DatabaseError> {
        let started_at = Timestamp::now();
        let finished_at = (!matches!(run.status, RunStatus::Running)).then(Timestamp::now);
        let (status_text, status_detail) = run.status.stored();
        let id: Id<Run> = database.ids.next();

        let mut transaction = database.write().await?;
        let project_id = project_of_snapshot(&mut transaction, run.snapshot_id).await?;

        sqlx::query(
            "INSERT INTO runs \
             (id, snapshot_id, analyzer, status, status_detail, compared_against, started_at, finished_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.raw())
        .bind(run.snapshot_id.raw())
        .bind(run.analyzer)
        .bind(status_text)
        .bind(status_detail)
        .bind(run.compared_against.map(Id::raw))
        .bind(started_at.to_string())
        .bind(finished_at.map(|at| at.to_string()))
        .execute(&mut *transaction)
        .await?;

        for finding in run.findings {
            let issue_id = resolve_issue(
                &mut transaction,
                &database.ids,
                project_id,
                run.analyzer,
                &finding.fingerprint,
                run.snapshot_id,
            )
            .await?;
            insert_finding(&mut transaction, &database.ids, id, issue_id, finding).await?;
        }

        for signal in run.signals {
            insert_signal(&mut transaction, &database.ids, id, signal).await?;
        }

        transaction.commit().await?;

        Ok(Run {
            id,
            snapshot_id: run.snapshot_id,
            analyzer: run.analyzer.to_owned(),
            status: run.status,
            compared_against: run.compared_against,
            started_at,
            finished_at,
        })
    }

    /// The baseline for a comparison: the newest successful run of this analyzer on any
    /// earlier snapshot of the same subject. A failed run is not a baseline, because
    /// "found nothing" and "could not look" are different answers.
    pub async fn last_successful(
        database: &Database,
        subject_id: Id<Subject>,
        analyzer: &str,
        before: Id<Snapshot>,
    ) -> Result<Option<Run>, DatabaseError> {
        let row = sqlx::query(
            "SELECT r.id, r.snapshot_id, r.status_detail, r.compared_against, r.started_at, r.finished_at \
             FROM snapshots s JOIN runs r ON r.snapshot_id = COALESCE(s.analysis_snapshot_id, s.id) \
             WHERE s.subject_id = ? AND r.analyzer = ? AND r.status = 'succeeded' AND s.id < ? \
             ORDER BY s.id DESC, r.id DESC LIMIT 1",
        )
        .bind(subject_id.raw())
        .bind(analyzer)
        .bind(before.raw())
        .fetch_optional(&database.pool)
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
            started_at: Timestamp::read(&row, "started_at")?,
            finished_at: row
                .try_get::<Option<String>, _>("finished_at")?
                .map(|value| Timestamp::from_stored(&value))
                .transpose()?,
        }))
    }

    fn decode_row(row: &SqliteRow, snapshot_id: Id<Snapshot>) -> Result<Run, DatabaseError> {
        Ok(Run {
            id: Id::from_raw(row.try_get("id")?),
            snapshot_id,
            analyzer: row.try_get("analyzer")?,
            status: RunStatus::from_stored(row.try_get("status")?, row.try_get("status_detail")?)?,
            compared_against: row
                .try_get::<Option<i64>, _>("compared_against")?
                .map(Id::from_raw),
            started_at: Timestamp::read(row, "started_at")?,
            finished_at: row
                .try_get::<Option<String>, _>("finished_at")?
                .map(|value| Timestamp::from_stored(&value))
                .transpose()?,
        })
    }
}

impl SnapshotAnalysis {
    pub async fn for_snapshot(
        database: &Database,
        project_id: Id<Project>,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Option<SnapshotAnalysis>, DatabaseError> {
        let row = sqlx::query(
            "SELECT s.id AS subject_id, s.project_id, s.kind AS subject_kind, s.subject_key, \
                    n.id, n.head, n.base, n.merge_base, n.forge_title, n.forge_body, n.forge_author, \
                    n.forge_url, n.forge_base_ref, n.forge_head_ref, n.forge_state, n.forge_merge_commit, \
                    n.signature_present, n.signature_verified, n.signature_signer, n.signature_reason, n.observed_at \
             FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND n.id = ?",
        )
        .bind(project_id.raw())
        .bind(snapshot_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(load_analysis(database, row).await?)),
            None => Ok(None),
        }
    }

    pub async fn page(
        database: &Database,
        project_id: Id<Project>,
        before: Option<Id<Snapshot>>,
        subject: Option<(&str, &str)>,
        latest: bool,
    ) -> Result<(Vec<SnapshotAnalysis>, Option<Id<Snapshot>>), DatabaseError> {
        let (kind, key) = subject.map_or((None, None), |(kind, key)| (Some(kind), Some(key)));
        let mut rows = sqlx::query(
            "SELECT s.id AS subject_id, s.project_id, s.kind AS subject_kind, s.subject_key, \
                    n.id, n.head, n.base, n.merge_base, n.forge_title, n.forge_body, n.forge_author, \
                    n.forge_url, n.forge_base_ref, n.forge_head_ref, n.forge_state, n.forge_merge_commit, \
                    n.signature_present, n.signature_verified, n.signature_signer, n.signature_reason, n.observed_at \
             FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND (? IS NULL OR n.id < ?) \
               AND (? IS NULL OR (s.kind = ? AND s.subject_key = ?)) \
               AND (? = 0 OR n.id = (SELECT MAX(newest.id) FROM snapshots newest WHERE newest.subject_id = s.id)) \
             ORDER BY n.id DESC LIMIT 51",
        )
        .bind(project_id.raw())
        .bind(before.map(Id::raw))
        .bind(before.map(Id::raw))
        .bind(kind)
        .bind(kind)
        .bind(key)
        .bind(latest)
        .fetch_all(&database.pool)
        .await?;

        let has_more = rows.len() > 50;
        if has_more {
            rows.pop();
        }
        let next_before = if has_more {
            rows.last()
                .map(|row| row.try_get::<i64, _>("id").map(Id::from_raw))
                .transpose()?
        } else {
            None
        };
        let analyses = load_page(database, rows).await?;
        Ok((analyses, next_before))
    }
}
async fn load_analysis(
    database: &Database,
    row: SqliteRow,
) -> Result<SnapshotAnalysis, DatabaseError> {
    let subject = Subject::decode_row(&row)?;
    let mut snapshot = Snapshot::decode_row(&row)?;
    snapshot.forge.people = snapshot.people(database).await?;
    let check_runs = CheckRun::for_snapshot(database, snapshot.id).await?;
    let runs = runs_for_snapshot(database, snapshot.id).await?;
    Ok(SnapshotAnalysis {
        subject,
        snapshot,
        check_runs,
        runs,
    })
}

async fn load_page(
    database: &Database,
    rows: Vec<SqliteRow>,
) -> Result<Vec<SnapshotAnalysis>, DatabaseError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut analyses = Vec::with_capacity(rows.len());
    for row in rows {
        analyses.push(SnapshotAnalysis {
            subject: Subject::decode_row(&row)?,
            snapshot: Snapshot::decode_row(&row)?,
            check_runs: Vec::new(),
            runs: Vec::new(),
        });
    }
    let ids = analyses
        .iter()
        .map(|analysis| analysis.snapshot.id.raw())
        .collect::<Vec<_>>();
    let positions = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect::<HashMap<_, _>>();

    let mut people_query = QueryBuilder::<Sqlite>::new(
        "SELECT snapshot_id, role, person_name, email, login, avatar_url FROM snapshot_people WHERE snapshot_id IN (",
    );
    {
        let mut separated = people_query.separated(", ");
        for id in &ids {
            separated.push_bind(id);
        }
    }
    let people_rows = people_query
        .push(") ORDER BY snapshot_id, id")
        .build()
        .fetch_all(&database.pool)
        .await?;
    for row in people_rows {
        let id: i64 = row.try_get("snapshot_id")?;
        let person = Person {
            role: PersonRole::read(&row, "role")?,
            name: row.try_get("person_name")?,
            email: row.try_get("email")?,
            login: row.try_get("login")?,
            avatar_url: row.try_get("avatar_url")?,
        };
        analyses[positions[&id]].snapshot.forge.people.push(person);
    }

    let mut checks_query = QueryBuilder::<Sqlite>::new(
        "SELECT n.id AS page_snapshot_id, c.name, c.status, c.conclusion, c.url, c.log_excerpt_ref \
         FROM snapshots n JOIN check_runs c ON c.snapshot_id = COALESCE(n.analysis_snapshot_id, n.id) WHERE n.id IN (",
    );
    {
        let mut separated = checks_query.separated(", ");
        for id in &ids {
            separated.push_bind(id);
        }
    }
    let check_rows = checks_query
        .push(") ORDER BY n.id, c.id")
        .build()
        .fetch_all(&database.pool)
        .await?;
    for row in check_rows {
        let id: i64 = row.try_get("page_snapshot_id")?;
        analyses[positions[&id]].check_runs.push(CheckRun {
            name: row.try_get("name")?,
            status: ci_checks::CheckStatus::read(&row, "status")?,
            conclusion: row
                .try_get::<Option<String>, _>("conclusion")?
                .map(|value| ci_checks::CheckConclusion::from_stored(&value))
                .transpose()?,
            url: row.try_get("url")?,
            log_excerpt_ref: row.try_get("log_excerpt_ref")?,
        });
    }

    let mut runs_query = QueryBuilder::<Sqlite>::new(
        "SELECT n.id AS page_snapshot_id, r.id, r.snapshot_id, r.analyzer, r.status, r.status_detail, \
                r.compared_against, r.started_at, r.finished_at \
         FROM snapshots n JOIN runs r ON r.snapshot_id = COALESCE(n.analysis_snapshot_id, n.id) WHERE n.id IN (",
    );
    {
        let mut separated = runs_query.separated(", ");
        for id in &ids {
            separated.push_bind(id);
        }
    }
    let run_rows = runs_query
        .push(") ORDER BY n.id, r.id")
        .build()
        .fetch_all(&database.pool)
        .await?;
    let mut run_positions = HashMap::new();
    for row in run_rows {
        let page_id: i64 = row.try_get("page_snapshot_id")?;
        let snapshot_id = Id::from_raw(row.try_get("snapshot_id")?);
        let run = Run::decode_row(&row, snapshot_id)?;
        let analysis_index = positions[&page_id];
        let run_index = analyses[analysis_index].runs.len();
        run_positions.insert(run.id.raw(), (analysis_index, run_index, snapshot_id));
        analyses[analysis_index].runs.push(CompletedRun {
            run,
            findings: Vec::new(),
            signals: Vec::new(),
        });
    }

    if !run_positions.is_empty() {
        let mut findings_query = QueryBuilder::<Sqlite>::new(
            "SELECT f.run_id, f.id, f.issue_id, f.location_kind, f.file_path, f.line_start, f.line_end, f.movement, \
                    f.ecosystem, f.package_name, f.package_version, f.origin_kind, f.origin_ref, \
                    f.package_integrity, f.severity, f.confidence, \
                    f.attribution, f.title, f.detail, \
                    i.fingerprint, i.fingerprint_version, i.fingerprint_canonical \
             FROM findings f JOIN issues i ON i.id = f.issue_id WHERE f.run_id IN (",
        );
        {
            let mut separated = findings_query.separated(", ");
            for run_id in run_positions.keys() {
                separated.push_bind(run_id);
            }
        }
        let finding_rows = findings_query
            .push(") ORDER BY f.run_id, f.id")
            .build()
            .fetch_all(&database.pool)
            .await?;
        for row in finding_rows {
            let run_id: i64 = row.try_get("run_id")?;
            let (analysis_index, run_index, _) = run_positions[&run_id];
            analyses[analysis_index].runs[run_index]
                .findings
                .push(Finding::decode_row(&row, Id::from_raw(run_id))?);
        }

        let mut signals_query = QueryBuilder::<Sqlite>::new(
            "SELECT run_id, signal_key, value_kind, score, flag, count_value, confidence, reason \
             FROM signals WHERE run_id IN (",
        );
        {
            let mut separated = signals_query.separated(", ");
            for run_id in run_positions.keys() {
                separated.push_bind(run_id);
            }
        }
        let signal_rows = signals_query
            .push(") ORDER BY run_id, id")
            .build()
            .fetch_all(&database.pool)
            .await?;
        for row in signal_rows {
            let run_id: i64 = row.try_get("run_id")?;
            let (analysis_index, run_index, snapshot_id) = run_positions[&run_id];
            analyses[analysis_index].runs[run_index]
                .signals
                .push(Signal::decode_row(&row, Id::from_raw(run_id), snapshot_id)?);
        }
    }

    Ok(analyses)
}

impl RunStatus {
    fn stored(&self) -> (&'static str, Option<String>) {
        match self {
            RunStatus::Running => ("running", None),
            RunStatus::Succeeded => ("succeeded", None),
            RunStatus::Failed { message } => ("failed", Some(message.clone())),
            RunStatus::TimedOut => ("timed_out", None),
            RunStatus::Skipped { reason } => ("skipped", Some(reason.clone())),
        }
    }

    fn from_stored(status: String, detail: Option<String>) -> Result<Self, DatabaseError> {
        match (status.as_str(), detail) {
            ("running", _) => Ok(RunStatus::Running),
            ("succeeded", _) => Ok(RunStatus::Succeeded),
            ("failed", Some(message)) => Ok(RunStatus::Failed { message }),
            ("timed_out", _) => Ok(RunStatus::TimedOut),
            ("skipped", Some(reason)) => Ok(RunStatus::Skipped { reason }),
            _ => Err(DatabaseError::Unreadable {
                field: "run_status",
                value: status,
            }),
        }
    }
}

pub fn confidence_from_stored(value: f64) -> Result<Confidence, DatabaseError> {
    Confidence::new(value as f32).ok_or(DatabaseError::Unreadable {
        field: "confidence",
        value: value.to_string(),
    })
}

async fn runs_for_snapshot(
    database: &Database,
    snapshot_id: Id<Snapshot>,
) -> Result<Vec<CompletedRun>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, snapshot_id, analyzer, status, status_detail, compared_against, started_at, finished_at \
         FROM runs WHERE snapshot_id = \
             (SELECT COALESCE(analysis_snapshot_id, id) FROM snapshots WHERE id = ?) ORDER BY id",
    )
    .bind(snapshot_id.raw())
    .fetch_all(&database.pool)
    .await?;

    let mut runs = Vec::with_capacity(rows.len());
    for row in rows {
        let snapshot_id = Id::from_raw(row.try_get("snapshot_id")?);
        let run = Run::decode_row(&row, snapshot_id)?;
        let findings = Finding::for_run(database, run.id).await?;
        let signals = Signal::for_run(database, run.id, snapshot_id).await?;
        runs.push(CompletedRun {
            run,
            findings,
            signals,
        });
    }

    Ok(runs)
}

async fn project_of_snapshot(
    connection: &mut SqliteConnection,
    snapshot_id: Id<Snapshot>,
) -> Result<Id<Project>, DatabaseError> {
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
) -> Result<Id<Issue>, DatabaseError> {
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
    let (triage, triage_reason) = Triage::Untriaged.stored();
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
) -> Result<(), DatabaseError> {
    let id: Id<Finding> = ids.next();
    let location = finding.location.encoded();

    sqlx::query(
        "INSERT INTO findings \
         (id, run_id, issue_id, location_kind, file_path, line_start, line_end, ecosystem, \
          package_name, package_version, origin_kind, origin_ref, package_integrity, \
          severity, confidence, attribution, movement, title, detail) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(location.origin_kind)
    .bind(location.origin_ref)
    .bind(location.package_integrity)
    .bind(finding.severity.stored())
    .bind(f64::from(finding.confidence.value()))
    .bind(finding.attribution.stored())
    .bind(finding.movement.map(|movement| movement.stored()))
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
) -> Result<(), DatabaseError> {
    let id: Id<Signal> = ids.next();
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
    .bind(signal.key.stored())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::finding::fingerprint::Components;
    use crate::analysis::finding::{Attribution, Location, NewFinding, Severity};
    use crate::analysis::signal::{NewSignal, SignalKey};
    use crate::forge::ForgeMetadata;
    use crate::project::subject::SubjectKind;

    #[tokio::test]
    async fn history_cursor_reaches_old_snapshots_and_subject_filter_precedes_limit() {
        let database = Database::open("sqlite::memory:", 0).await.expect("opens");
        let project_id: Id<Project> = database.ids.next();
        sqlx::query(
            "INSERT INTO projects (id, name, remote_url, forge_kind, uses_default_analyzers) \
             VALUES (?, 'history', 'https://github.com/owner/repo', 'auto', 1)",
        )
        .bind(project_id.raw())
        .execute(&database.pool)
        .await
        .expect("project is stored");
        let old_subject = Subject::upsert(&database, project_id, SubjectKind::Change { number: 1 })
            .await
            .expect("old subject is stored");
        let other_subject =
            Subject::upsert(&database, project_id, SubjectKind::Change { number: 2 })
                .await
                .expect("other subject is stored");
        let old = Snapshot::record(
            &database,
            old_subject.id,
            CommitSha::new(&format!("{:040x}", 1)).expect("sha"),
            None,
            None,
            ForgeMetadata::default(),
        )
        .await
        .expect("old snapshot is stored");
        for number in 2..=52 {
            Snapshot::record(
                &database,
                other_subject.id,
                CommitSha::new(&format!("{number:040x}")).expect("sha"),
                None,
                None,
                ForgeMetadata::default(),
            )
            .await
            .expect("new snapshot is stored");
        }
        let location = Location::Package {
            path: RepoPath::new("flake.lock").expect("valid path"),
            ecosystem: Ecosystem::Nix,
            name: "nixpkgs".to_owned(),
            version: "abc".to_owned(),
            origin: Some(PackageOrigin::Remote {
                url: "https://github.com/NixOS/nixpkgs/tree/abc".to_owned(),
            }),
            integrity: Some("sha256-abc".to_owned()),
        };
        let finding = NewFinding {
            movement: None,
            fingerprint: Fingerprint::compute(&Components {
                analyzer: "secret-scan",
                rule: "credential",
                location: &location,
                title: "Credential found",
                occurrence: 0,
            }),
            location,
            severity: Severity::High,
            confidence: Confidence::new(1.0).expect("valid confidence"),
            attribution: Attribution::Introduced,
            title: "Credential found".to_owned(),
            detail: "A credential-shaped value was added".to_owned(),
        };
        let signal = NewSignal {
            key: SignalKey::TestsFailing,
            value: SignalValue::Flag(true),
            confidence: Confidence::new(1.0).expect("valid confidence"),
            reason: "A check failed".to_owned(),
        };
        Run::record(
            &database,
            NewRun {
                snapshot_id: old.id,
                analyzer: "secret-scan",
                status: RunStatus::Succeeded,
                compared_against: None,
                findings: &[finding],
                signals: &[signal],
            },
        )
        .await
        .expect("records analysis details");

        let (first, next) = SnapshotAnalysis::page(&database, project_id, None, None, false)
            .await
            .expect("first page loads");
        assert_eq!(first.len(), 50);
        let (second, end) = SnapshotAnalysis::page(&database, project_id, next, None, false)
            .await
            .expect("second page loads");
        assert_eq!(second.len(), 2);
        assert_eq!(
            second.last().map(|analysis| analysis.snapshot.id),
            Some(old.id)
        );
        assert!(end.is_none());
        let old_page = second.last().expect("old snapshot is on second page");
        assert_eq!(old_page.runs[0].findings[0].title, "Credential found");
        assert!(matches!(
            &old_page.runs[0].findings[0].location,
            Location::Package { origin: Some(PackageOrigin::Remote { url }), .. }
                if url == "https://github.com/NixOS/nixpkgs/tree/abc"
        ));
        assert_eq!(old_page.runs[0].signals[0].value, SignalValue::Flag(true));

        let (filtered, end) =
            SnapshotAnalysis::page(&database, project_id, None, Some(("change", "1")), false)
                .await
                .expect("subject history loads");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].snapshot.id, old.id);
        assert!(end.is_none());
        let (latest, end) = SnapshotAnalysis::page(&database, project_id, None, None, true)
            .await
            .expect("latest subject page loads");
        assert_eq!(latest.len(), 2);
        assert_eq!(
            latest.last().map(|analysis| analysis.snapshot.id),
            Some(old.id)
        );
        assert!(end.is_none());
        let direct = SnapshotAnalysis::for_snapshot(&database, project_id, old.id)
            .await
            .expect("snapshot loads")
            .expect("snapshot exists");
        assert_eq!(direct.snapshot.id, old.id);
    }
}
