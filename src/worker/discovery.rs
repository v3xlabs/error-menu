use crate::analysis::runner::{self, Analysis, AnalysisError, Target};
use crate::app::AppState;
use crate::forge::reader::ForgeReadError;
use crate::forge::{ChangeState, CommitReading, DiscoveredChange, ForgeAccount, ForgeMetadata};
use crate::prelude::*;
use crate::vcs::mirror::{Mirror, MirrorError};

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
    #[error("database: {0}")]
    Database(#[from] DatabaseError),
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

pub async fn run(state: &AppState, project_id: Id<Project>) -> Result<Discovery, DiscoveryError> {
    let _project_lock = runner::project_lock(project_id).await;
    let project = Project::load(&state.database, project_id)
        .await?
        .ok_or(DiscoveryError::ProjectNotFound)?;
    let discovered = state
        .forge
        .discover(&project.remote, project.forge_kind)
        .await?;
    let mut mirror = None;
    let default_head = discovered.default_branch.head;
    let default_head_for_icon = default_head.clone();
    let default_name = discovered.default_branch.name;
    let indexed = Snapshot::indexed(&state.database, project.id, &default_head).await?;
    let analysis = match indexed {
        Some(indexed) => {
            let subject = Subject::upsert(
                &state.database,
                project.id,
                SubjectKind::Branch {
                    name: default_name.clone(),
                },
            )
            .await?;
            let snapshot = Snapshot::observe(
                &state.database,
                subject.id,
                default_head,
                None,
                ForgeMetadata::default(),
                Some(&indexed),
            )
            .await?;
            Analysis::for_snapshot(&state.database, snapshot).await?
        }
        None => {
            let mirror = mirror_for_analysis(&mut mirror, state, &project).await?;
            let default_base = mirror
                .first_parent(&default_head)
                .await?
                .ok_or(DiscoveryError::DefaultBranchRoot)?;
            let reading = read_commit(state, &project, mirror, &default_head).await?;
            runner::run_target_in_mirror(
                state,
                &project,
                mirror,
                Target {
                    subject: SubjectKind::Branch {
                        name: default_name.clone(),
                    },
                    base: default_base,
                    forge: ForgeMetadata {
                        people: people_of(mirror, &default_head, Vec::new(), &reading.accounts)
                            .await?,
                        signature: reading.signature,
                        ..ForgeMetadata::default()
                    },
                    head: default_head,
                },
            )
            .await?
        }
    };
    let default_branch = BranchAnalysis {
        name: default_name,
        analysis,
    };

    let mut changes = Vec::with_capacity(discovered.changes.len());
    for change in discovered.changes {
        changes.push(read_change(state, &project, &mut mirror, change).await?);
    }
    if project.icon == ProjectIcon::default()
        && let Some(mirror) = mirror.as_ref()
    {
        let icon =
            crate::project::icon::infer(mirror, &default_head_for_icon, &project.name).await?;
        project
            .describe(&state.database, project.description.as_deref(), &icon)
            .await?;
    }

    Ok(Discovery {
        default_branch,
        changes,
    })
}

async fn mirror_for_analysis<'a>(
    mirror: &'a mut Option<Mirror>,
    state: &AppState,
    project: &Project,
) -> Result<&'a Mirror, MirrorError> {
    match mirror {
        Some(mirror) => Ok(mirror),
        None => {
            let opened = Mirror::open(&state.mirrors, &project.remote).await?;
            opened.fetch().await?;
            Ok(mirror.insert(opened))
        }
    }
}

async fn read_change(
    state: &AppState,
    project: &Project,
    mirror: &mut Option<Mirror>,
    change: DiscoveredChange,
) -> Result<ChangeAnalysis, DiscoveryError> {
    let indexed = Snapshot::indexed(&state.database, project.id, &change.head).await?;
    if change.metadata.state == Some(ChangeState::Open) && indexed.is_none() {
        let mirror = mirror_for_analysis(mirror, state, project).await?;
        return analyse_change(state, project, mirror, change).await;
    }

    let subject = Subject::upsert(
        &state.database,
        project.id,
        SubjectKind::Change {
            number: change.number,
        },
    )
    .await?;
    let snapshot = Snapshot::observe(
        &state.database,
        subject.id,
        change.head,
        Some(change.base),
        change.metadata,
        indexed.as_ref(),
    )
    .await?;

    Ok(ChangeAnalysis {
        number: change.number,
        metadata: snapshot.forge.clone(),
        snapshot,
    })
}

async fn analyse_change(
    state: &AppState,
    project: &Project,
    mirror: &Mirror,
    change: DiscoveredChange,
) -> Result<ChangeAnalysis, DiscoveryError> {
    mirror.fetch_ref(&change.fetch_ref).await?;

    let number = change.number;
    let reading = read_commit(state, project, mirror, &change.head).await?;
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
        state,
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
    state: &AppState,
    project: &Project,
    mirror: &Mirror,
    head: &CommitSha,
) -> Result<CommitReading, DiscoveryError> {
    let signed = mirror.commit_signature(head).await?;
    let reading = state
        .forge
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

    Ok(crate::project::person::deduplicate(named_by_forge(
        people, accounts,
    )))
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

/// Explicitly reruns a recorded change using its forge ref, even if its head is indexed.
pub async fn scan_change(
    state: &AppState,
    project_id: Id<Project>,
    number: u64,
) -> Result<Snapshot, DiscoveryError> {
    let _project_lock = runner::project_lock(project_id).await;
    let project = Project::load(&state.database, project_id)
        .await?
        .ok_or(DiscoveryError::ProjectNotFound)?;
    let recorded = Snapshot::latest_for_change(&state.database, project_id, number)
        .await?
        .ok_or(DiscoveryError::ChangeNotFound)?;
    let base = recorded
        .base
        .clone()
        .ok_or(DiscoveryError::ChangeNotFound)?;

    let mirror = Mirror::open(&state.mirrors, &project.remote).await?;
    mirror
        .fetch_ref(&crate::forge::change_fetch_ref(project.forge_kind, number))
        .await?;

    let reading = read_commit(state, &project, &mirror, &recorded.head).await?;
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
        state,
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
