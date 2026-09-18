use std::path::Path;

use crate::analysis::runner::{self, Analysis, AnalysisError, Target};
use crate::forge::reader::{ForgeReadError, ForgeReader};
use crate::forge::{ChangeState, CommitReading, DiscoveredChange, ForgeAccount, ForgeMetadata};
use crate::id::Id;
use crate::person::{Person, Signature};
use crate::store::{Store, StoreError};
use crate::vcs::CommitSha;
use crate::vcs::mirror::{Mirror, MirrorError};
use crate::watch::{Project, Snapshot, SubjectKind};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("project was not found")]
    ProjectNotFound,
    #[error("forge: {0}")]
    Forge(#[from] ForgeReadError),
    #[error("repository: {0}")]
    Repository(#[from] MirrorError),
    #[error("analysis: {0}")]
    Analysis(#[from] AnalysisError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("that change has not been discovered yet")]
    ChangeNotFound,
    #[error("the default branch has no parent commit")]
    DefaultBranchRoot,
}

pub struct Discovery {
    pub default_branch: BranchAnalysis,
    pub changes: Vec<ChangeAnalysis>,
}

pub struct BranchAnalysis {
    pub name: String,
    pub analysis: Analysis,
}

pub struct ChangeAnalysis {
    pub number: u64,
    pub metadata: ForgeMetadata,
    pub snapshot: Snapshot,
}

pub async fn run(
    store: &Store,
    mirror_root: &Path,
    project_id: Id<Project>,
) -> Result<Discovery, DiscoveryError> {
    let project = store
        .project(project_id)
        .await?
        .ok_or(DiscoveryError::ProjectNotFound)?;
    let reader = ForgeReader::new()?;
    let discovered = reader.discover(&project.remote, project.forge_kind).await?;
    let mirror = Mirror::open(mirror_root, &project.remote).await?;
    mirror.fetch().await?;

    let default_head = discovered.default_branch.head;
    let default_head_for_icon = default_head.clone();
    let default_base = mirror
        .first_parent(&default_head)
        .await?
        .ok_or(DiscoveryError::DefaultBranchRoot)?;
    let default_reading = read_commit(&reader, &project, &mirror, &default_head).await?;
    let default_branch = BranchAnalysis {
        name: discovered.default_branch.name.clone(),
        analysis: runner::run_target_in_mirror(
            store,
            &project,
            &mirror,
            Target {
                subject: SubjectKind::Branch {
                    name: discovered.default_branch.name,
                },
                base: default_base,
                forge: ForgeMetadata {
                    people: people_of(
                        &mirror,
                        &default_head,
                        Vec::new(),
                        &default_reading.accounts,
                    )
                    .await?,
                    signature: default_reading.signature,
                    ..ForgeMetadata::default()
                },
                head: default_head,
            },
        )
        .await?,
    };

    if project.icon == crate::watch::ProjectIcon::default() {
        let icon = crate::icon::infer(&mirror, &default_head_for_icon, &project.name).await?;
        store
            .describe_project(project.id, project.description.as_deref(), &icon)
            .await?;
    }

    let mut changes = Vec::with_capacity(discovered.changes.len());
    for change in discovered.changes {
        changes.push(read_change(store, &reader, &project, &mirror, change).await?);
    }

    Ok(Discovery {
        default_branch,
        changes,
    })
}

/// A change that is still open is fetched and put through every gate. A change that is
/// already closed or merged is recorded for its title, its people, and the commit that
/// merged it, because scanning work that nobody can act on costs a fetch and a gate run
/// for every change the repository has ever had.
async fn read_change(
    store: &Store,
    reader: &ForgeReader,
    project: &Project,
    mirror: &Mirror,
    change: DiscoveredChange,
) -> Result<ChangeAnalysis, DiscoveryError> {
    if change.metadata.state == Some(ChangeState::Open) {
        return analyse_change(store, reader, project, mirror, change).await;
    }

    let subject = store
        .upsert_subject(
            project.id,
            SubjectKind::Change {
                number: change.number,
            },
        )
        .await?;
    let snapshot = store
        .record_snapshot(
            subject.id,
            change.head,
            Some(change.base),
            None,
            change.metadata.clone(),
        )
        .await?;

    Ok(ChangeAnalysis {
        number: change.number,
        metadata: change.metadata,
        snapshot,
    })
}

async fn analyse_change(
    store: &Store,
    reader: &ForgeReader,
    project: &Project,
    mirror: &Mirror,
    change: DiscoveredChange,
) -> Result<ChangeAnalysis, DiscoveryError> {
    mirror.fetch_ref(&change.fetch_ref).await?;

    let number = change.number;
    let reading = read_commit(reader, project, mirror, &change.head).await?;
    let metadata = ForgeMetadata {
        people: people_of(
            mirror,
            &change.head,
            change.metadata.people,
            &reading.accounts,
        )
        .await?,
        signature: reading.signature,
        ..change.metadata
    };
    let analysis = runner::run_target_in_mirror(
        store,
        project,
        mirror,
        Target {
            subject: SubjectKind::Change { number },
            base: change.base,
            head: change.head,
            forge: metadata.clone(),
        },
    )
    .await?;

    Ok(ChangeAnalysis {
        number,
        metadata,
        snapshot: analysis.snapshot,
    })
}

/// A signature the commit carries but the forge will not vouch for is reported as
/// present and unverified. error.menu never checks the cryptography itself.
///
/// The same request carries the accounts that wrote the commit, so it runs whether or not
/// the commit is signed: without it, one human arrives twice, once from the forge with a
/// login and once from the commit with an address.
async fn read_commit(
    reader: &ForgeReader,
    project: &Project,
    mirror: &Mirror,
    head: &CommitSha,
) -> Result<CommitReading, DiscoveryError> {
    let signed = mirror.commit_signature(head).await?;
    let reading = reader
        .read_commit(&project.remote, project.forge_kind, head)
        .await
        .unwrap_or_default();

    Ok(CommitReading {
        signature: if signed {
            Signature {
                present: true,
                ..reading.signature
            }
        } else {
            Signature::default()
        },
        accounts: reading.accounts,
    })
}

/// The forge knows who opened a change and who was asked to review it. The commit knows
/// who wrote it and whom its trailers credit. Neither alone is the whole answer.
async fn people_of(
    mirror: &Mirror,
    head: &CommitSha,
    from_forge: Vec<Person>,
    accounts: &[ForgeAccount],
) -> Result<Vec<Person>, MirrorError> {
    let mut people = from_forge;
    people.extend(mirror.commit_people(head).await?);

    Ok(crate::person::deduplicate(named_by_forge(people, accounts)))
}

/// A commit is written with an address and a forge names people by login, so the same human
/// arrives from the two sources with nothing in common. The forge's own commit payload says
/// which account owns the address, and that is what makes the two one person.
fn named_by_forge(people: Vec<Person>, accounts: &[ForgeAccount]) -> Vec<Person> {
    people
        .into_iter()
        .map(|person| {
            let Some(email) = person.email.as_deref() else {
                return person;
            };
            let Some(account) = accounts
                .iter()
                .find(|account| account.email.eq_ignore_ascii_case(email))
            else {
                return person;
            };

            Person {
                login: person.login.or_else(|| Some(account.login.clone())),
                avatar_url: person.avatar_url.or_else(|| account.avatar_url.clone()),
                ..person
            }
        })
        .collect()
}

/// Scans one change that was recorded but never put through the gates. Its head lives
/// behind the forge's own ref, which the mirror does not carry until it is asked for.
pub async fn scan_change(
    store: &Store,
    mirror_root: &Path,
    project_id: Id<Project>,
    number: u64,
) -> Result<Snapshot, DiscoveryError> {
    let project = store
        .project(project_id)
        .await?
        .ok_or(DiscoveryError::ProjectNotFound)?;
    let recorded = store
        .latest_snapshot_for_change(project_id, number)
        .await?
        .ok_or(DiscoveryError::ChangeNotFound)?;
    let base = recorded
        .base
        .clone()
        .ok_or(DiscoveryError::ChangeNotFound)?;

    let reader = ForgeReader::new()?;
    let mirror = Mirror::open(mirror_root, &project.remote).await?;
    mirror
        .fetch_ref(&crate::forge::change_fetch_ref(project.forge_kind, number))
        .await?;

    let reading = read_commit(&reader, &project, &mirror, &recorded.head).await?;
    let metadata = ForgeMetadata {
        people: people_of(
            &mirror,
            &recorded.head,
            recorded.forge.people.clone(),
            &reading.accounts,
        )
        .await?,
        signature: reading.signature,
        ..recorded.forge
    };
    let analysis = runner::run_target_in_mirror(
        store,
        &project,
        &mirror,
        Target {
            subject: SubjectKind::Change { number },
            base,
            head: recorded.head,
            forge: metadata,
        },
    )
    .await?;

    Ok(analysis.snapshot)
}
