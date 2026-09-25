//! What error.menu tells a forge about a commit of a project that reports to it. The
//! verdict and the stored state are the forge's business only through the transport: a
//! verdict is error.menu's own answer, and the row keeps which forge check carries it.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use jiff::Timestamp;
use sqlx::Row;
use tokio::sync::Mutex;

use crate::app::AppState;
use crate::database::codec::{FromStored, StoredAs};
use crate::forge::github::app::{GithubApp, GithubAppError};
use crate::forge::github::check::{self, Request};
use crate::forge::github::installation::{GithubInstallation, full_name};
use crate::prelude::*;

/// A snapshot's findings, as far as the forge is told. Only a finding the change
/// introduced and nobody has triaged counts: a muted or acknowledged issue was already
/// decided by a person, and a preexisting one is not this change's doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Scanned {
        new_findings: BTreeMap<Severity, u64>,
        failed_analyzers: u64,
    },
    /// The scan itself stopped. That is error.menu's failure, not the change's.
    Unfinished,
}

/// How the forge should colour a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Pass,
    Attention,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Queued,
    InProgress,
    Completed,
}

/// Where a scan of one head has got to.
#[derive(Clone, Copy)]
pub enum Step<'a> {
    Queued,
    Started,
    /// The analysis of `snapshot` is stored; its verdict is read from there.
    Analysed {
        subject: &'a SubjectKind,
        snapshot_id: Id<Snapshot>,
    },
    /// The scan stopped and will be tried again.
    Unfinished {
        subject: &'a SubjectKind,
    },
    /// A head analysed before: its check is completed from the stored analysis, unless it
    /// already carries a verdict. This catches a check that a lost webhook or a refused
    /// write left open, and a head analysed before the project began to report.
    Settled {
        subject: &'a SubjectKind,
        snapshot_id: Id<Snapshot>,
    },
}

/// A project whose verdicts go to its forge: the switch is on, the server has a GitHub
/// App, and an installation of that app covers the repository.
pub struct Reporting<'a> {
    app: &'a GithubApp,
    installation: GithubInstallation,
    repository: String,
    project_id: Id<Project>,
}

#[derive(Debug, thiserror::Error)]
enum ReportError {
    #[error("{0}")]
    Database(#[from] DatabaseError),
    #[error("{0}")]
    Github(#[from] GithubAppError),
}

struct ForgeCheck {
    check_id: u64,
    state: CheckState,
}

impl Verdict {
    /// The verdict of the analysis a snapshot points at. A snapshot of a head that was
    /// analysed before carries no runs of its own and reads the analysed one's.
    pub async fn for_snapshot(
        database: &Database,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Verdict, DatabaseError> {
        let rows = sqlx::query(
            "SELECT f.severity, COUNT(*) AS count \
             FROM findings f JOIN runs r ON r.id = f.run_id JOIN issues i ON i.id = f.issue_id \
             WHERE r.snapshot_id = \
                 (SELECT COALESCE(analysis_snapshot_id, id) FROM snapshots WHERE id = ?) \
             AND f.attribution = 'introduced' AND i.triage = 'untriaged' \
             GROUP BY f.severity",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&database.pool)
        .await?;
        let mut new_findings = BTreeMap::new();
        for row in rows {
            let count: i64 = row.try_get("count")?;
            new_findings.insert(Severity::read(&row, "severity")?, count.unsigned_abs());
        }
        let failed_analyzers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs WHERE snapshot_id = \
                 (SELECT COALESCE(analysis_snapshot_id, id) FROM snapshots WHERE id = ?) \
             AND status IN ('failed', 'timed_out')",
        )
        .bind(snapshot_id.raw())
        .fetch_one(&database.pool)
        .await?;

        Ok(Verdict::Scanned {
            new_findings,
            failed_analyzers: failed_analyzers.unsigned_abs(),
        })
    }

    /// Red for a new High or Critical finding from any analyzer, green for a change that
    /// introduced nothing, and grey for everything between, including a scan that could
    /// not finish.
    pub fn tone(&self) -> Tone {
        match self {
            Verdict::Unfinished => Tone::Attention,
            Verdict::Scanned {
                new_findings,
                failed_analyzers,
            } => {
                if new_findings
                    .iter()
                    .any(|(severity, count)| *severity >= Severity::High && *count > 0)
                {
                    Tone::Fail
                } else if new_findings.values().all(|count| *count == 0) && *failed_analyzers == 0 {
                    Tone::Pass
                } else {
                    Tone::Attention
                }
            }
        }
    }
}

impl<'a> Reporting<'a> {
    pub async fn for_project(
        state: &'a AppState,
        project: &Project,
    ) -> Result<Option<Reporting<'a>>, DatabaseError> {
        let (true, Some(app), Some(repository)) = (
            project.reports_to_forge,
            state.github_app.as_deref(),
            full_name(&project.remote, project.forge_kind),
        ) else {
            return Ok(None);
        };
        let Some(installation) = GithubInstallation::for_project(&state.database, project).await?
        else {
            return Ok(None);
        };

        Ok(Some(Reporting {
            app,
            installation,
            repository,
            project_id: project.id,
        }))
    }

    /// Tells the forge where a scan of `head` has got to. A forge that refuses is logged and
    /// left: the scan result is already stored, and the next pass that finds the head
    /// without a completed check writes it again.
    pub async fn record(&self, database: &Database, head: &CommitSha, step: Step<'_>) {
        if let Err(error) = self.write(database, head, step).await {
            tracing::warn!(
                project = %self.project_id,
                %head,
                %error,
                "could not update the forge check"
            );
        }
    }

    /// Drops the check kept on `head`, so the next pass writes a new one. A re-run asked
    /// for on the forge resets the check there, and a head with nothing new to scan would
    /// otherwise keep answering with the check that was reset.
    pub async fn forget(&self, database: &Database, head: &CommitSha) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM forge_checks WHERE project_id = ? AND head = ?")
            .bind(self.project_id.raw())
            .bind(head.as_str())
            .execute(&database.pool)
            .await?;

        Ok(())
    }

    async fn write(
        &self,
        database: &Database,
        head: &CommitSha,
        step: Step<'_>,
    ) -> Result<(), ReportError> {
        // A webhook and a scan can reach the same head at once. Without one writer at a
        // time both would find no check and each create one, and one of the two would
        // stay queued on the forge for ever.
        static WRITER: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
        let _writer = WRITER.lock().await;

        let existing = ForgeCheck::load(database, self.project_id, head).await?;
        let open = existing
            .as_ref()
            .filter(|check| check.state != CheckState::Completed);
        let request = match step {
            // A head that already has a check is covered, whatever state it is in.
            Step::Queued if existing.is_some() => return Ok(()),
            Step::Settled { .. } if existing.is_some() && open.is_none() => return Ok(()),
            Step::Started if open.is_some_and(|check| check.state == CheckState::InProgress) => {
                return Ok(());
            }
            Step::Queued => Request::Queued,
            Step::Started => Request::InProgress,
            Step::Analysed {
                subject,
                snapshot_id,
            }
            | Step::Settled {
                subject,
                snapshot_id,
            } => Request::Completed {
                verdict: Verdict::for_snapshot(database, snapshot_id).await?,
                details_url: self.details_url(subject, head),
            },
            Step::Unfinished { subject } => Request::Completed {
                verdict: Verdict::Unfinished,
                details_url: self.details_url(subject, head),
            },
        };
        let token = self
            .app
            .installation_token(self.installation.id, &self.repository)
            .await?;
        let check_id = match open {
            Some(check) => {
                check::update(self.app, &token, &self.repository, check.check_id, &request).await?;
                check.check_id
            }
            None => check::create(self.app, &token, &self.repository, head, &request).await?,
        };

        ForgeCheck::save(database, self.project_id, head, check_id, request.state()).await?;
        Ok(())
    }

    /// The subject page at the scan of this head, which is what the check is about. Without
    /// the head the page shows the newest scan, and a link on an older commit would open a
    /// later one. The page needs a sign-in, so the detail behind the counts stays here.
    fn details_url(&self, subject: &SubjectKind, head: &CommitSha) -> String {
        let (kind, key) = subject.stored();
        let project = self.project_id.encode();
        let mut url = self.app.public_origin().clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.clear()
                .extend(["projects", project.as_str(), kind, key.as_str()]);
        }
        url.query_pairs_mut()
            .clear()
            .append_pair("head", head.as_str());

        url.to_string()
    }
}

impl ForgeCheck {
    async fn load(
        database: &Database,
        project_id: Id<Project>,
        head: &CommitSha,
    ) -> Result<Option<ForgeCheck>, DatabaseError> {
        let row = sqlx::query(
            "SELECT check_id, state FROM forge_checks WHERE project_id = ? AND head = ?",
        )
        .bind(project_id.raw())
        .bind(head.as_str())
        .fetch_optional(&database.pool)
        .await?;

        row.map(|row| {
            let check_id: i64 = row.try_get("check_id")?;
            Ok(ForgeCheck {
                check_id: check_id.unsigned_abs(),
                state: CheckState::read(&row, "state")?,
            })
        })
        .transpose()
    }

    async fn save(
        database: &Database,
        project_id: Id<Project>,
        head: &CommitSha,
        check_id: u64,
        state: CheckState,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO forge_checks (project_id, head, check_id, state, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(project_id, head) DO UPDATE SET \
             check_id = excluded.check_id, state = excluded.state, updated_at = excluded.updated_at",
        )
        .bind(project_id.raw())
        .bind(head.as_str())
        .bind(i64::try_from(check_id).map_err(|_| DatabaseError::Unreadable {
            field: "forge_checks.check_id",
            value: check_id.to_string(),
        })?)
        .bind(state.stored())
        .bind(Timestamp::now().to_string())
        .execute(&database.pool)
        .await?;

        Ok(())
    }
}

impl StoredAs for CheckState {
    fn stored(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

impl FromStored for CheckState {
    const FIELD: &'static str = "forge_checks.state";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(findings: &[(Severity, u64)], failed_analyzers: u64) -> Verdict {
        Verdict::Scanned {
            new_findings: findings.iter().copied().collect(),
            failed_analyzers,
        }
    }

    #[test]
    fn only_a_new_high_or_critical_finding_turns_the_check_red() {
        assert_eq!(scanned(&[(Severity::Critical, 1)], 0).tone(), Tone::Fail);
        assert_eq!(
            scanned(&[(Severity::High, 1), (Severity::Low, 3)], 2).tone(),
            Tone::Fail
        );
        assert_eq!(scanned(&[(Severity::Medium, 4)], 0).tone(), Tone::Attention);
        assert_eq!(scanned(&[(Severity::Info, 1)], 0).tone(), Tone::Attention);
    }

    #[test]
    fn a_clean_scan_passes_and_a_scan_that_could_not_look_does_not() {
        assert_eq!(scanned(&[], 0).tone(), Tone::Pass);
        assert_eq!(scanned(&[], 1).tone(), Tone::Attention);
        assert_eq!(Verdict::Unfinished.tone(), Tone::Attention);
    }
}
