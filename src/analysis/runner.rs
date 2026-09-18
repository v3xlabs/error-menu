use std::collections::BTreeMap;
use std::path::Path;

use crate::analysis::{
    Run, RunStatus, ci_checks, hygiene, links, lockfile, manifest, repository_controls, secret,
    workflow,
};
use crate::forge::ForgeMetadata;
use crate::forge::reader::{ForgeReadError, ForgeReader};
use crate::id::Id;
use crate::store::{RunRecord, Store, StoreError};
use crate::vcs::CommitSha;
use crate::vcs::mirror::{ChangedFile, FileChange, Mirror, MirrorError};
use crate::watch::{Project, Snapshot, SubjectKind};

const MAX_SECRET_SCAN_BYTES: u64 = 32 * 1024 * 1024;
pub const DEFAULT_ANALYZERS: [&str; 8] = [
    lockfile::ANALYZER,
    manifest::ANALYZER,
    secret::ANALYZER,
    links::ANALYZER,
    workflow::ANALYZER,
    repository_controls::ANALYZER,
    ci_checks::ANALYZER,
    hygiene::ANALYZER,
];

pub fn is_known_analyzer(analyzer: &str) -> bool {
    DEFAULT_ANALYZERS.contains(&analyzer)
}
pub fn effective_analyzers(uses_default_analyzers: bool, custom_analyzers: Vec<String>) -> Vec<String> {
    if uses_default_analyzers {
        return DEFAULT_ANALYZERS.into_iter().map(str::to_owned).collect();
    }

    custom_analyzers
}



#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("project was not found")]
    ProjectNotFound,
    #[error("commit has no parent")]
    RootCommit,
    #[error("repository: {0}")]
    Repository(#[from] MirrorError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

pub struct Analysis {
    pub snapshot: Snapshot,
    pub analyzers: Vec<AnalyzerRun>,
}

pub struct AnalyzerRun {
    pub run: Run,
    pub finding_count: u64,
    pub signal_count: u64,
}

pub struct Target {
    pub subject: SubjectKind,
    pub base: CommitSha,
    pub head: CommitSha,
    pub forge: ForgeMetadata,
}

/// Analyses one commit range. Without a base, the comparison is against the commit's own
/// first parent, so a reader who has one commit in hand does not have to find its parent.
pub async fn run(
    store: &Store,
    mirror_root: &Path,
    project_id: Id<Project>,
    base: Option<CommitSha>,
    head: CommitSha,
) -> Result<Analysis, AnalysisError> {
    let project = store
        .project(project_id)
        .await?
        .ok_or(AnalysisError::ProjectNotFound)?;
    let mirror = Mirror::open(mirror_root, &project.remote).await?;
    mirror.fetch().await?;
    let base = match base {
        Some(base) => base,
        None => mirror
            .first_parent(&head)
            .await?
            .ok_or(AnalysisError::RootCommit)?,
    };

    run_target_in_mirror(
        store,
        &project,
        &mirror,
        Target {
            subject: SubjectKind::Commit { sha: head.clone() },
            base,
            head,
            forge: ForgeMetadata::default(),
        },
    )
    .await
}

pub(crate) async fn run_target_in_mirror(
    store: &Store,
    project: &Project,
    mirror: &Mirror,
    target: Target,
) -> Result<Analysis, AnalysisError> {
    let merge_base = mirror.merge_base(&target.base, &target.head).await?;
    let changed_files = mirror.changed_files(&target.base, &target.head).await?;
    let analyzers = effective_analyzers(
        project.uses_default_analyzers,
        if project.uses_default_analyzers {
            Vec::new()
        } else {
            store.custom_project_analyzers(project.id).await?
        },
    );
    let subject = store.upsert_subject(project.id, target.subject).await?;
    let snapshot = store
        .record_snapshot(
            subject.id,
            target.head.clone(),
            Some(target.base.clone()),
            Some(merge_base),
            target.forge,
        )
        .await?;
    let check_runs = analyzers
        .iter()
        .any(|analyzer| analyzer == ci_checks::ANALYZER)
        .then(|| check_run_input(project, &target.head));
    let check_runs = match check_runs {
        Some(input) => Some(input.await),
        None => None,
    };
    if let Some(CheckRunInput::Ready(check_runs)) = check_runs.as_ref() {
        store.replace_check_runs(snapshot.id, check_runs).await?;
    }

    let mut completed = Vec::new();
    for analyzer in analyzers {
        let previous = store
            .last_successful_analyzer(subject.id, &analyzer, snapshot.id)
            .await?;
        let result = evaluate(
            &analyzer,
            mirror,
            &target.base,
            &target.head,
            &changed_files,
            check_runs.as_ref(),
        )
        .await;
        let (status, findings, signals) = match result {
            Ok((findings, signals)) => (RunStatus::Succeeded, findings, signals),
            Err(AnalyzerError::CiNotConfigured) => (
                RunStatus::Skipped {
                    reason: "CI checks need a supported configured forge".to_owned(),
                },
                Vec::new(),
                Vec::new(),
            ),
            Err(error) => (
                RunStatus::Failed {
                    message: error.to_string(),
                },
                Vec::new(),
                Vec::new(),
            ),
        };
        let finding_count = findings.len() as u64;
        let signal_count = signals.len() as u64;
        let run = store
            .record_run(RunRecord {
                snapshot_id: snapshot.id,
                analyzer: &analyzer,
                status,
                compared_against: previous.map(|run| run.snapshot_id),
                findings: &findings,
                signals: &signals,
            })
            .await?;

        completed.push(AnalyzerRun {
            run,
            finding_count,
            signal_count,
        });
    }

    Ok(Analysis {
        snapshot,
        analyzers: completed,
    })
}

async fn check_run_input(project: &Project, head: &CommitSha) -> CheckRunInput {
    let result = async {
        let reader = ForgeReader::new()?;
        reader
            .check_runs(&project.remote, project.forge_kind, head)
            .await
    }
    .await;

    match result {
        Ok(check_runs) => CheckRunInput::Ready(check_runs),
        Err(ForgeReadError::ForgeType) => CheckRunInput::NotConfigured,
        Err(error) => CheckRunInput::Failed(error.to_string()),
    }
}

async fn evaluate(
    analyzer: &str,
    mirror: &Mirror,
    base: &CommitSha,
    head: &CommitSha,
    changed_files: &[ChangedFile],
    check_runs: Option<&CheckRunInput>,
) -> Result<
    (
        Vec<crate::finding::NewFinding>,
        Vec<crate::signal::NewSignal>,
    ),
    AnalyzerError,
> {
    match analyzer {
        lockfile::ANALYZER => Ok((
            lockfile::delta_for_change(mirror, base, head).await?,
            Vec::new(),
        )),
        manifest::ANALYZER => Ok((
            manifest_findings(mirror, base, head, changed_files).await?,
            Vec::new(),
        )),
        secret::ANALYZER => Ok((
            secret_findings(mirror, base, head, changed_files).await?,
            Vec::new(),
        )),
        links::ANALYZER => {
            let base_links = inventory_links(mirror, base).await?;
            let head_links = inventory_links(mirror, head).await?;
            let report = links::report(&base_links, &head_links);

            Ok((report.findings, report.signals))
        }
        workflow::ANALYZER => Ok((
            workflow_findings(mirror, base, head, changed_files).await?,
            Vec::new(),
        )),
        repository_controls::ANALYZER => Ok((
            repository_control_findings(mirror, base, head, changed_files).await?,
            Vec::new(),
        )),
        ci_checks::ANALYZER => {
            let Some(check_runs) = check_runs else {
                return Err(AnalyzerError::CiNotConfigured);
            };
            let checks = match check_runs {
                CheckRunInput::Ready(checks) => checks,
                CheckRunInput::NotConfigured => return Err(AnalyzerError::CiNotConfigured),
                CheckRunInput::Failed(message) => return Err(AnalyzerError::CiRead(message.clone())),
            };
            let classification = ci_checks::classify(checks);

            Ok((Vec::new(), classification.signal.into_iter().collect()))
        }
        hygiene::ANALYZER => Ok((
            Vec::new(),
            hygiene_signals(mirror, head, changed_files).await?,
        )),
        other => Err(AnalyzerError::Unavailable(other.to_owned())),
    }
}

async fn manifest_findings(
    mirror: &Mirror,
    base: &CommitSha,
    head: &CommitSha,
    changed_files: &[ChangedFile],
) -> Result<Vec<crate::finding::NewFinding>, AnalyzerError> {
    let mut findings = Vec::new();

    for changed in changed_files {
        if !manifest::supports(&changed.path) {
            continue;
        }
        let base_text = mirror.file_at(base, &changed.path).await?;
        let head_text = mirror.file_at(head, &changed.path).await?;
        findings.extend(manifest::delta(
            &changed.path,
            base_text.as_deref(),
            head_text.as_deref(),
        )?);
    }

    Ok(findings)
}

async fn secret_findings(
    mirror: &Mirror,
    base: &CommitSha,
    head: &CommitSha,
    changed_files: &[ChangedFile],
) -> Result<Vec<crate::finding::NewFinding>, AnalyzerError> {
    let mut scanned_bytes: u64 = 0;
    let mut findings = Vec::new();

    for changed in changed_files {
        if changed.change == FileChange::Deleted {
            continue;
        }
        let Some(size) = mirror.file_size_at(head, &changed.path).await? else {
            continue;
        };
        scanned_bytes = scanned_bytes.saturating_add(size);
        if scanned_bytes > MAX_SECRET_SCAN_BYTES {
            return Err(AnalyzerError::SecretScanLimit);
        }

        let head_text = match mirror.file_at(head, &changed.path).await {
            Ok(Some(text)) => text,
            Ok(None) | Err(MirrorError::NotText { .. }) => continue,
            Err(error) => return Err(error.into()),
        };
        let base_text = match mirror.file_at(base, &changed.path).await {
            Ok(text) => text,
            Err(MirrorError::NotText { .. }) => None,
            Err(error) => return Err(error.into()),
        };
        findings.extend(secret::added_line_findings(
            &changed.path,
            base_text.as_deref(),
            &head_text,
        ));
    }

    Ok(findings)
}

async fn inventory_links(
    mirror: &Mirror,
    revision: &CommitSha,
) -> Result<Vec<links::Link>, AnalyzerError> {
    let mut inventory = Vec::new();

    for path in mirror.paths_at(revision, usize::MAX).await? {
        match mirror.file_at(revision, &path).await {
            Ok(Some(text)) => inventory.extend(links::inventory(&path, &text)),
            Ok(None) | Err(MirrorError::NotText { .. }) => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(inventory)
}

async fn workflow_findings(
    mirror: &Mirror,
    base: &CommitSha,
    head: &CommitSha,
    changed_files: &[ChangedFile],
) -> Result<Vec<crate::finding::NewFinding>, AnalyzerError> {
    let mut findings = Vec::new();

    for changed in changed_files {
        if changed.change == FileChange::Deleted || !workflow::is_workflow(&changed.path) {
            continue;
        }
        let Some(head_text) = mirror.file_at(head, &changed.path).await? else {
            continue;
        };
        let base_text = mirror.file_at(base, &changed.path).await?;
        findings.extend(workflow::delta(
            &changed.path,
            base_text.as_deref(),
            &head_text,
        ));
    }

    Ok(findings)
}

async fn repository_control_findings(
    mirror: &Mirror,
    base: &CommitSha,
    head: &CommitSha,
    changed_files: &[ChangedFile],
) -> Result<Vec<crate::finding::NewFinding>, AnalyzerError> {
    let mut findings = Vec::new();

    for changed in changed_files {
        if repository_controls::kind(&changed.path).is_none() {
            continue;
        }
        let base_text = mirror.file_at(base, &changed.path).await?;
        let head_text = mirror.file_at(head, &changed.path).await?;
        findings.extend(
            repository_controls::report(&changed.path, base_text.as_deref(), head_text.as_deref())
                .findings,
        );
    }

    Ok(findings)
}

async fn hygiene_signals(
    mirror: &Mirror,
    head: &CommitSha,
    changed_files: &[ChangedFile],
) -> Result<Vec<crate::signal::NewSignal>, AnalyzerError> {
    let mut sizes = BTreeMap::new();
    for changed in changed_files {
        if changed.change != FileChange::Deleted {
            sizes.insert(
                changed.path.clone(),
                mirror.file_size_at(head, &changed.path).await?,
            );
        }
    }

    Ok(hygiene::signals(changed_files, |path| {
        sizes.get(path).copied().flatten()
    }))
}

enum CheckRunInput {
    Ready(Vec<ci_checks::CheckRun>),
    NotConfigured,
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
enum AnalyzerError {
    #[error("lockfile: {0}")]
    Lockfile(#[from] lockfile::LockfileError),
    #[error("manifest: {0}")]
    Manifest(#[from] manifest::ManifestError),
    #[error("repository: {0}")]
    Repository(#[from] MirrorError),
    #[error("CI checks: {0}")]
    CiRead(String),
    #[error("CI checks need a supported configured forge")]
    CiNotConfigured,
    #[error("the secret scan exceeds its {MAX_SECRET_SCAN_BYTES} byte input limit")]
    SecretScanLimit,
    #[error("analyzer {0} is not implemented")]
    Unavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_uses_every_implemented_analyzer() {
        let analyzers = effective_analyzers(true, vec![secret::ANALYZER.to_owned()]);

        assert_eq!(analyzers, DEFAULT_ANALYZERS.map(str::to_owned));
        assert!(analyzers.iter().all(|analyzer| is_known_analyzer(analyzer)));
    }

    #[test]
    fn custom_mode_keeps_only_its_saved_analyzers() {
        let custom = vec![secret::ANALYZER.to_owned(), workflow::ANALYZER.to_owned()];

        assert_eq!(effective_analyzers(false, custom.clone()), custom);
    }
}
