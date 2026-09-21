//! Gitea, and Forgejo which forked it. Both answer GitHub's API shape closely enough that
//! a commit reading is GitHub's, but they report CI as commit statuses rather than checks.

use serde::Deserialize;

use super::github::Github;
use super::reader::{Api, ApiBase, ForgeReadError, PAGE_SIZE, optional_sha, sha};
use super::{
    ChangeState, CommitReading, DiscoveredChange, Forge, ForgeMetadata, check_conclusion,
    check_status,
};
use crate::analysis::ci_checks::CheckRun;
use crate::prelude::*;

pub(crate) struct Gitea;

impl Forge for Gitea {
    fn api_base(_host: &str, authority: &str) -> Result<ApiBase, ForgeReadError> {
        ApiBase::new(&format!("https://{authority}/"), &["api", "v1"])
    }

    fn change_ref(number: u64) -> String {
        format!("refs/pull/{number}/head")
    }

    async fn changes(api: &Api<'_>) -> Result<Vec<DiscoveredChange>, ForgeReadError> {
        let changes: Vec<GiteaChange> = api
            .get_with_query(
                api.repository.repository_url("repos", &["pulls"]),
                &[
                    ("state", "all"),
                    ("sort", "recentupdate"),
                    ("limit", PAGE_SIZE),
                ],
            )
            .await?;

        changes.into_iter().map(TryInto::try_into).collect()
    }

    async fn read_commit(api: &Api<'_>, head: &CommitSha) -> Result<CommitReading, ForgeReadError> {
        Github::read_commit(api, head).await
    }

    async fn check_runs(api: &Api<'_>, head: &CommitSha) -> Result<Vec<CheckRun>, ForgeReadError> {
        let response: GiteaCombinedStatus = api
            .get(
                api.repository
                    .repository_url("repos", &["commits", head.as_str(), "status"]),
            )
            .await?;

        Ok(response
            .statuses
            .into_iter()
            .map(GiteaCommitStatus::into_check_run)
            .collect())
    }
}

#[derive(Deserialize)]
struct GiteaCombinedStatus {
    #[serde(default)]
    statuses: Vec<GiteaCommitStatus>,
}

#[derive(Deserialize)]
struct GiteaCommitStatus {
    context: String,
    status: String,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl GiteaCommitStatus {
    fn into_check_run(self) -> CheckRun {
        let log_excerpt_ref = self.url;
        let url = self.target_url.or_else(|| log_excerpt_ref.clone());

        CheckRun {
            name: self.context,
            status: check_status(&self.status),
            conclusion: check_conclusion(&self.status),
            url,
            log_excerpt_ref,
        }
    }
}

#[derive(Deserialize)]
struct GiteaChange {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    state: String,
    #[serde(default)]
    merged: bool,
    merge_commit_sha: Option<String>,
    user: GiteaUser,
    #[serde(default)]
    requested_reviewers: Vec<GiteaUser>,
    base: GiteaReference,
    head: GiteaReference,
}

#[derive(Deserialize)]
struct GiteaUser {
    login: String,
    full_name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GiteaReference {
    sha: String,
    #[serde(rename = "ref")]
    name: String,
}

impl GiteaUser {
    fn person(self, role: PersonRole) -> Person {
        Person {
            role,
            name: self.full_name.filter(|name| !name.is_empty()),
            email: None,
            login: Some(self.login),
            avatar_url: self.avatar_url,
        }
    }
}

impl TryFrom<GiteaChange> for DiscoveredChange {
    type Error = ForgeReadError;

    fn try_from(change: GiteaChange) -> Result<Self, Self::Error> {
        let state = if change.merged {
            ChangeState::Merged
        } else if change.state == "closed" {
            ChangeState::Closed
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
            fetch_ref: Gitea::change_ref(change.number),
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
