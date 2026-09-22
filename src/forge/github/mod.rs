//! GitHub, and the commit shape the forges that copy its API answer with. Gitea and
//! Forgejo read commits through this module rather than describing the same payload twice.

use serde::Deserialize;

use super::reader::{Api, ApiBase, Credential, ForgeReadError, PAGE_SIZE, optional_sha, sha};
use super::{
    ChangeState, CommitReading, DiscoveredChange, Forge, ForgeAccount, ForgeMetadata,
    check_conclusion, check_status,
};
use crate::analysis::ci_checks::CheckRun;
use crate::prelude::*;

/// The public GitHub REST API answers on its own host, so a credential for it is keyed
/// here and not under the host a repository is cloned from.
pub const API_HOST: &str = "api.github.com";

pub(crate) struct Github;

impl Forge for Github {
    /// GitHub Enterprise Server serves the same API under the repository's own host.
    fn api_base(host: &str, authority: &str) -> Result<ApiBase, ForgeReadError> {
        match host {
            "github.com" => ApiBase::new("https://api.github.com/", &[]),
            _ => ApiBase::new(&format!("https://{authority}/"), &["api", "v3"]),
        }
    }

    fn change_ref(number: u64) -> String {
        format!("refs/pull/{number}/head")
    }

    /// The OAuth app that signs users in reads public data on its own budget of five
    /// thousand an hour, against the sixty a source address gets anonymously. It is the
    /// same registration either way, so a deployment that can sign users in can already
    /// read at that rate.
    fn client_credential() -> Option<(String, Credential)> {
        let (id, secret) = std::env::var("GITHUB_CLIENT_ID")
            .ok()
            .zip(std::env::var("GITHUB_CLIENT_SECRET").ok())
            .filter(|(id, secret)| !id.is_empty() && !secret.is_empty())?;

        Some((API_HOST.to_owned(), Credential::ClientApp { id, secret }))
    }

    async fn changes(api: &Api<'_>) -> Result<Vec<DiscoveredChange>, ForgeReadError> {
        let pulls: Vec<GithubPull> = api
            .get_with_query(
                api.repository.repository_url("repos", &["pulls"]),
                &[
                    ("state", "all"),
                    ("sort", "updated"),
                    ("direction", "desc"),
                    ("per_page", PAGE_SIZE),
                ],
            )
            .await?;

        pulls.into_iter().map(TryInto::try_into).collect()
    }

    async fn read_commit(api: &Api<'_>, head: &CommitSha) -> Result<CommitReading, ForgeReadError> {
        let commit: CommitDetail = api
            .get(
                api.repository
                    .repository_url("repos", &["commits", head.as_str()]),
            )
            .await?;
        let verification = commit.commit.verification;

        Ok(CommitReading {
            signature: Signature {
                present: verification
                    .as_ref()
                    .is_some_and(|value| value.signature.is_some()),
                verified: verification.as_ref().map(|value| value.verified),
                signer: verification
                    .as_ref()
                    .and_then(|value| value.signer.as_ref())
                    .map(|signer| signer.login.clone()),
                reason: verification.and_then(|value| value.reason),
            },
            accounts: [
                account_of(commit.commit.author, commit.author),
                account_of(commit.commit.committer, commit.committer),
            ]
            .into_iter()
            .flatten()
            .collect(),
        })
    }

    async fn check_runs(api: &Api<'_>, head: &CommitSha) -> Result<Vec<CheckRun>, ForgeReadError> {
        let response: GithubCheckRuns = api
            .get_with_query(
                api.repository
                    .repository_url("repos", &["commits", head.as_str(), "check-runs"]),
                &[("per_page", PAGE_SIZE)],
            )
            .await?;

        Ok(response
            .check_runs
            .into_iter()
            .map(GithubCheckRun::into_check_run)
            .collect())
    }
}

#[derive(Deserialize)]
struct GithubCheckRuns {
    #[serde(default)]
    check_runs: Vec<GithubCheckRun>,
}

#[derive(Deserialize)]
struct GithubCheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl GithubCheckRun {
    fn into_check_run(self) -> CheckRun {
        let log_excerpt_ref = self.url;
        let url = self.details_url.or_else(|| log_excerpt_ref.clone());

        CheckRun {
            name: self.name,
            status: check_status(&self.status),
            conclusion: self.conclusion.as_deref().and_then(check_conclusion),
            url,
            log_excerpt_ref,
        }
    }
}

#[derive(Deserialize)]
struct GithubPull {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    state: String,
    #[serde(default)]
    draft: bool,
    merged_at: Option<String>,
    merge_commit_sha: Option<String>,
    user: GithubUser,
    #[serde(default)]
    requested_reviewers: Vec<GithubUser>,
    base: GithubReference,
    head: GithubReference,
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GithubReference {
    sha: String,
    #[serde(rename = "ref")]
    name: String,
}

impl GithubUser {
    fn person(self, role: PersonRole) -> Person {
        Person {
            role,
            name: None,
            email: None,
            login: Some(self.login),
            avatar_url: self.avatar_url,
        }
    }
}

impl TryFrom<GithubPull> for DiscoveredChange {
    type Error = ForgeReadError;

    fn try_from(change: GithubPull) -> Result<Self, Self::Error> {
        let state = if change.merged_at.is_some() {
            ChangeState::Merged
        } else if change.state == "closed" {
            ChangeState::Closed
        } else if change.draft {
            ChangeState::Draft
        } else {
            ChangeState::Open
        };
        let mut people = vec![change.user.person(PersonRole::Submitter)];
        people.extend(
            change
                .requested_reviewers
                .into_iter()
                .map(|user| user.person(PersonRole::Reviewer)),
        );

        Ok(Self {
            number: change.number,
            fetch_ref: Github::change_ref(change.number),
            base: sha("pull request base", &change.base.sha)?,
            head: sha("pull request head", &change.head.sha)?,
            metadata: ForgeMetadata {
                title: Some(change.title),
                body: change.body,
                author: people.first().and_then(|person| person.login.clone()),
                url: Some(change.html_url),
                base_ref: Some(change.base.name),
                head_ref: Some(change.head.name),
                state: Some(state),
                merge_commit: optional_sha(change.merge_commit_sha.as_deref()),
                people: crate::project::person::deduplicate(people),
                signature: Signature::default(),
            },
        })
    }
}

#[derive(Deserialize)]
struct CommitDetail {
    commit: CommitBody,
    author: Option<CommitAccount>,
    committer: Option<CommitAccount>,
}

#[derive(Deserialize)]
struct CommitBody {
    verification: Option<CommitVerification>,
    author: Option<CommitIdentity>,
    committer: Option<CommitIdentity>,
}

#[derive(Deserialize)]
struct CommitIdentity {
    email: Option<String>,
}

#[derive(Deserialize)]
struct CommitAccount {
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct CommitVerification {
    verified: bool,
    reason: Option<String>,
    signature: Option<String>,
    signer: Option<CommitSigner>,
}

#[derive(Deserialize)]
struct CommitSigner {
    login: String,
}

/// One account, only when the forge gave both halves of the join: the address the commit
/// carries and the account it belongs to.
fn account_of(
    identity: Option<CommitIdentity>,
    account: Option<CommitAccount>,
) -> Option<ForgeAccount> {
    let email = identity?.email?;
    let account = account?;

    Some(ForgeAccount {
        email,
        login: account.login,
        avatar_url: account.avatar_url,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn state_of(state: &str, draft: bool, merged_at: Option<&str>) -> ChangeState {
        let pull: GithubPull = serde_json::from_value(json!({
            "number": 7,
            "html_url": "https://github.com/owner/repository/pull/7",
            "title": "a change",
            "body": null,
            "state": state,
            "draft": draft,
            "merged_at": merged_at,
            "merge_commit_sha": null,
            "user": { "login": "author", "avatar_url": null },
            "base": { "sha": "1".repeat(40), "ref": "main" },
            "head": { "sha": "2".repeat(40), "ref": "work" },
        }))
        .expect("the payload matches what the pulls endpoint answers with");

        DiscoveredChange::try_from(pull)
            .expect("both shas are well formed")
            .metadata
            .state
            .expect("a pull request always has a state")
    }

    #[test]
    fn draft_yields_to_the_outcome_a_change_reached() {
        assert_eq!(state_of("open", true, None), ChangeState::Draft);
        assert_eq!(state_of("open", false, None), ChangeState::Open);
        assert_eq!(state_of("closed", true, None), ChangeState::Closed);
        assert_eq!(
            state_of("closed", true, Some("2026-09-22T00:00:00Z")),
            ChangeState::Merged
        );
    }
}
