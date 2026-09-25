//! The check run error.menu keeps on a GitHub commit. It carries counts and a link, never a
//! finding's title or location: on a public repository anyone can read a check.

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::forge::github::API_HOST;
use crate::forge::github::app::{GithubApp, GithubAppError};
use crate::forge::report::{CheckState, Tone, Verdict};
use crate::prelude::*;

/// The name the checks list shows. GitHub keeps the newest run of a name per commit, so a
/// re-scan replaces the row a reader sees rather than adding one.
pub const NAME: &str = "error.menu";

pub enum Request {
    Queued,
    InProgress,
    Completed {
        verdict: Verdict,
        details_url: String,
    },
}

#[derive(Serialize)]
struct Body<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sha: Option<&'a str>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    conclusion: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Output>,
}

#[derive(Serialize)]
struct Output {
    title: String,
    summary: String,
}

#[derive(Deserialize)]
struct Created {
    id: u64,
}

impl Request {
    pub fn state(&self) -> CheckState {
        match self {
            Request::Queued => CheckState::Queued,
            Request::InProgress => CheckState::InProgress,
            Request::Completed { .. } => CheckState::Completed,
        }
    }

    fn body<'a>(&'a self, head: Option<&'a CommitSha>) -> Body<'a> {
        let (status, conclusion, details_url, output) = match self {
            Request::Queued => ("queued", None, None, None),
            Request::InProgress => ("in_progress", None, None, None),
            Request::Completed {
                verdict,
                details_url,
            } => (
                "completed",
                Some(match verdict.tone() {
                    Tone::Pass => "success",
                    Tone::Attention => "neutral",
                    Tone::Fail => "failure",
                }),
                Some(details_url.as_str()),
                Some(output(verdict)),
            ),
        };

        Body {
            name: head.map(|_| NAME),
            head_sha: head.map(CommitSha::as_str),
            status,
            conclusion,
            details_url,
            output,
        }
    }
}

pub async fn create(
    app: &GithubApp,
    token: &str,
    repository: &str,
    head: &CommitSha,
    request: &Request,
) -> Result<u64, GithubAppError> {
    let created: Created = app
        .send(
            app.client()
                .post(check_runs_url(repository, None))
                .json(&request.body(Some(head))),
            token,
        )
        .await?;

    Ok(created.id)
}

pub async fn update(
    app: &GithubApp,
    token: &str,
    repository: &str,
    check_id: u64,
    request: &Request,
) -> Result<(), GithubAppError> {
    let _: IgnoredAny = app
        .send(
            app.client()
                .patch(check_runs_url(repository, Some(check_id)))
                .json(&request.body(None)),
            token,
        )
        .await?;

    Ok(())
}

fn check_runs_url(repository: &str, check_id: Option<u64>) -> reqwest::Url {
    let mut url = reqwest::Url::parse(&format!("https://{API_HOST}/"))
        .expect("the GitHub API base is a valid URL");
    let id = check_id.map(|id| id.to_string());
    if let Ok(mut path) = url.path_segments_mut() {
        path.clear().push("repos");
        path.extend(repository.split('/'));
        path.push("check-runs");
        path.extend(id.as_deref());
    }

    url
}

fn output(verdict: &Verdict) -> Output {
    let Verdict::Scanned {
        new_findings,
        failed_analyzers,
    } = verdict
    else {
        return Output {
            title: "error.menu could not finish the scan".to_owned(),
            summary: "The scan stopped before every analyzer ran. error.menu tries again on its \
                      next pass."
                .to_owned(),
        };
    };
    let total: u64 = new_findings.values().sum();
    let title = match total {
        0 => "No new findings".to_owned(),
        1 => "1 new finding".to_owned(),
        count => format!("{count} new findings"),
    };
    let mut lines: Vec<String> = new_findings
        .iter()
        .rev()
        .filter(|(_, count)| **count > 0)
        .map(|(severity, count)| format!("- {count} {}", severity_name(*severity)))
        .collect();
    if *failed_analyzers > 0 {
        lines.push(format!(
            "- {failed_analyzers} analyzer{} could not finish",
            if *failed_analyzers == 1 { "" } else { "s" }
        ));
    }
    lines.push(String::new());
    lines.push(
        "Only findings this change introduced and nobody has triaged are counted. \
         Open error.menu for the details."
            .to_owned(),
    );

    Output {
        title,
        summary: lines.join("\n"),
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_is_addressed_under_its_own_repository() {
        assert_eq!(
            check_runs_url("owner/repository", None).as_str(),
            "https://api.github.com/repos/owner/repository/check-runs"
        );
        assert_eq!(
            check_runs_url("owner/repository", Some(42)).as_str(),
            "https://api.github.com/repos/owner/repository/check-runs/42"
        );
    }
}
