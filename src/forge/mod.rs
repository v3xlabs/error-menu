pub mod gitea;
pub mod github;
pub mod gitlab;
pub mod reader;

use serde::{Deserialize, Serialize};

use crate::analysis::ci_checks::{CheckConclusion, CheckRun, CheckStatus};
use crate::database::codec::{FromStored, StoredAs};
use crate::forge::gitea::Gitea;
use crate::forge::github::Github;
use crate::forge::gitlab::Gitlab;
use crate::forge::reader::{Api, ApiBase, Credential, ForgeReadError};
use crate::prelude::*;

/// What a forge answers on top of git. Everything git itself publishes is read from the
/// mirror, so a forge is only asked for what it invents: where its API lives, the changes
/// open on a repository, whether it vouches for a commit's signature, and what its CI
/// made of one. Each implementation owns its own payloads and its own URL shapes.
pub(crate) trait Forge {
    /// Where this forge answers for a repository cloned from `host`. `authority` carries
    /// the port when the remote named one.
    fn api_base(host: &str, authority: &str) -> Result<ApiBase, ForgeReadError>;

    /// Where this forge publishes the head of a change.
    fn change_ref(number: u64) -> String;

    /// A credential this forge offers of its own, and the host to send it to. Most have
    /// none. An app registered with a forge can raise that forge's budget without asking
    /// any person for a permission over their account.
    fn client_credential() -> Option<(String, Credential)> {
        None
    }

    async fn changes(api: &Api<'_>) -> Result<Vec<DiscoveredChange>, ForgeReadError>;

    async fn read_commit(api: &Api<'_>, head: &CommitSha) -> Result<CommitReading, ForgeReadError>;

    async fn check_runs(api: &Api<'_>, head: &CommitSha) -> Result<Vec<CheckRun>, ForgeReadError>;
}

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

/// A change's lifecycle as the forge tells it. Draft is an open change its author has not
/// offered for review; a change that closed or merged while still a draft reports the
/// outcome, because that is what the forge shows and what a reader acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    Draft,
    Open,
    Closed,
    Merged,
}

impl StoredAs for ForgeKind {
    fn stored(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Gitea => "gitea",
            Self::Forgejo => "forgejo",
        }
    }
}

impl FromStored for ForgeKind {
    const FIELD: &'static str = "forge_kind";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "github" => Some(Self::Github),
            "gitlab" => Some(Self::Gitlab),
            "gitea" => Some(Self::Gitea),
            "forgejo" => Some(Self::Forgejo),
            _ => None,
        }
    }
}

impl StoredAs for ChangeState {
    fn stored(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
}

impl FromStored for ChangeState {
    const FIELD: &'static str = "forge_state";

    fn parse_stored(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "merged" => Some(Self::Merged),
            _ => None,
        }
    }
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
pub struct DiscoveredChange {
    pub number: u64,
    pub fetch_ref: String,
    pub base: CommitSha,
    pub head: CommitSha,
    pub metadata: ForgeMetadata,
}

/// Where a forge publishes the head of a change, asked of the forge itself.
pub fn change_fetch_ref(kind: ForgeKind, number: u64) -> String {
    match kind {
        ForgeKind::Gitlab => Gitlab::change_ref(number),
        ForgeKind::Gitea | ForgeKind::Forgejo => Gitea::change_ref(number),
        _ => Github::change_ref(number),
    }
}

pub(crate) fn check_status(value: &str) -> CheckStatus {
    match value {
        "queued" | "pending" | "created" | "scheduled" => CheckStatus::Queued,
        "completed" | "success" | "failure" | "failed" | "error" | "warning" | "neutral"
        | "skipped" | "cancelled" | "canceled" | "timed_out" | "action_required" | "stale" => {
            CheckStatus::Completed
        }
        _ => CheckStatus::InProgress,
    }
}

pub(crate) fn check_conclusion(value: &str) -> Option<CheckConclusion> {
    match value {
        "success" => Some(CheckConclusion::Success),
        "failure" | "failed" | "error" => Some(CheckConclusion::Failure),
        "warning" | "neutral" => Some(CheckConclusion::Neutral),
        "skipped" => Some(CheckConclusion::Skipped),
        "cancelled" | "canceled" => Some(CheckConclusion::Cancelled),
        "timed_out" => Some(CheckConclusion::TimedOut),
        "action_required" => Some(CheckConclusion::ActionRequired),
        "stale" => Some(CheckConclusion::Stale),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_provider_statuses_without_losing_failed_outcomes() {
        assert_eq!(check_status("pending"), CheckStatus::Queued);
        assert_eq!(check_status("running"), CheckStatus::InProgress);
        assert_eq!(check_status("failed"), CheckStatus::Completed);
        assert_eq!(check_conclusion("failed"), Some(CheckConclusion::Failure));
        assert_eq!(
            check_conclusion("canceled"),
            Some(CheckConclusion::Cancelled)
        );
        assert_eq!(check_conclusion("running"), None);
    }
}
