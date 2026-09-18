pub mod reader;

use serde::{Deserialize, Serialize};

use crate::person::{Person, Signature};
use crate::vcs::CommitSha;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeKind {
    #[default]
    Auto,
    Github,
    Gitlab,
    Gitea,
    Forgejo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeMetadata {
    pub title: Option<String>,
    pub body: Option<String>,
    pub author: Option<String>,
    pub url: Option<String>,
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
    pub state: Option<ChangeState>,
    pub merge_commit: Option<CommitSha>,
    pub people: Vec<Person>,
    pub signature: Signature,
}

/// A forge account matched to the address a commit was written with. The forge's own commit
/// payload is the only place that mapping exists, and without it a commit author and the
/// account that opened the change are two strangers who happen to be one person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeAccount {
    pub email: String,
    pub login: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitReading {
    pub signature: Signature,
    pub accounts: Vec<ForgeAccount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProject {
    pub default_branch: DiscoveredBranch,
    pub changes: Vec<DiscoveredChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredBranch {
    pub name: String,
    pub head: CommitSha,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredChange {
    pub number: u64,
    pub fetch_ref: String,
    pub base: CommitSha,
    pub head: CommitSha,
    pub metadata: ForgeMetadata,
}

/// Where a forge publishes the head of a change. GitLab calls them merge requests and
/// names the ref accordingly; the others agree on `refs/pull`.
pub fn change_fetch_ref(kind: ForgeKind, number: u64) -> String {
    match kind {
        ForgeKind::Gitlab => format!("refs/merge-requests/{number}/head"),
        _ => format!("refs/pull/{number}/head"),
    }
}
