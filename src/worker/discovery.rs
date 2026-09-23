use jiff::Timestamp;

use crate::analysis::runner::{self, Analysis, AnalysisError, Target};
use crate::app::AppState;
use crate::forge::reader::ForgeReadError;
use crate::forge::{
    ChangeState, CommitReading, DiscoveredChange, ForgeAccount, ForgeMetadata, moved_remote,
};
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

impl DiscoveryError {
    /// The forge budget this failure is waiting on, wherever it was raised. An analyzer
    /// reads the forge as well, so a wait can arrive wrapped in an analysis failure, and a
    /// caller that recognises one shape only would treat a wait as a broken project.
    pub fn rate_limited(&self) -> Option<(&str, Timestamp)> {
        let forge = match self {
            Self::Forge(error) => error,
            Self::Analysis(AnalysisError::Forge(error)) => error,
            _ => return None,
        };

        match forge {
            ForgeReadError::RateLimited { host, reset } => Some((host, *reset)),
            _ => None,
        }
    }
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
    let mut project = Project::load(&state.database, project_id)
        .await?
        .ok_or(DiscoveryError::ProjectNotFound)?;
    let mirror = Mirror::open(&state.mirrors, &project.remote).await?;
    let mut fetched = false;
    let default = mirror.default_branch().await?;
    let default_head = default.head;
    let default_head_for_icon = default_head.clone();
    let default_name = default.name;
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
            state.activity.observe(project.id, &snapshot);
            Analysis::for_snapshot(&state.database, snapshot).await?
        }
        None => {
            let mirror = with_objects(&mirror, &mut fetched).await?;
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

    let discovered = state
        .forge
        .changes(&project.remote, project.forge_kind)
        .await?;
    if let Some(remote) = moved_remote(&project.remote, &discovered) {
        project.repoint(&state.database, remote).await?;
    }
    let mut changes = Vec::with_capacity(discovered.len());
    for change in discovered {
        changes.push(read_change(state, &project, &mirror, &mut fetched, change).await?);
    }
    if project.icon == ProjectIcon::default() && fetched {
        let icon =
            crate::project::icon::infer(&mirror, &default_head_for_icon, &project.name).await?;
        project
            .describe(&state.database, project.description.as_deref(), &icon)
            .await?;
    }

    Ok(Discovery {
        default_branch,
        changes,
    })
}

/// The mirror with the remote's objects in it. A poll that finds every head already
/// analysed reads no object at all, so the pack is received at most once in a pass, and
/// only once something has to be read out of it.
async fn with_objects<'a>(
    mirror: &'a Mirror,
    fetched: &mut bool,
) -> Result<&'a Mirror, MirrorError> {
    if !*fetched {
        mirror.fetch().await?;
        *fetched = true;
    }

    Ok(mirror)
}

async fn read_change(
    state: &AppState,
    project: &Project,
    mirror: &Mirror,
    fetched: &mut bool,
    change: DiscoveredChange,
) -> Result<ChangeAnalysis, DiscoveryError> {
    let indexed = Snapshot::indexed(&state.database, project.id, &change.head).await?;
    let open = matches!(
        change.metadata.state,
        Some(ChangeState::Open | ChangeState::Draft)
    );
    if open && indexed.is_none() {
        let mirror = with_objects(mirror, fetched).await?;
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
    state.activity.observe(project.id, &snapshot);

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
    // An exhausted budget is a wait, not an answer. Recorded as an absence it would key an
    // analysis to this head that nothing computes again.
    let reading = match state
        .forge
        .read_commit(&project.remote, project.forge_kind, head)
        .await
    {
        Ok(reading) => reading,
        Err(error @ ForgeReadError::RateLimited { .. }) => return Err(error.into()),
        Err(_) => CommitReading::default(),
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    fn exhausted() -> ForgeReadError {
        ForgeReadError::RateLimited {
            host: "api.github.com".to_owned(),
            reset: Timestamp::from_second(3600).unwrap(),
        }
    }

    #[test]
    fn a_budget_wait_is_found_whether_discovery_or_an_analyzer_ran_into_it() {
        let waiting = Some(("api.github.com", Timestamp::from_second(3600).unwrap()));

        assert_eq!(DiscoveryError::Forge(exhausted()).rate_limited(), waiting);
        assert_eq!(
            DiscoveryError::Analysis(AnalysisError::Forge(exhausted())).rate_limited(),
            waiting
        );
        assert_eq!(
            DiscoveryError::Forge(ForgeReadError::ForgeType).rate_limited(),
            None
        );
        assert_eq!(DiscoveryError::ProjectNotFound.rate_limited(), None);
    }
}
