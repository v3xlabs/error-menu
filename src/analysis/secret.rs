use std::collections::BTreeSet;

use crate::analysis::finding::fingerprint::{Components, Fingerprint};
use crate::prelude::*;

pub const ANALYZER: &str = "secret-scan";

const AWS_ACCESS_KEY_PREFIX: &str = "AKIA";
const AWS_ACCESS_KEY_LENGTH: usize = 20;
const GITHUB_TOKEN_PREFIXES: [&str; 3] = ["ghp_", "github_pat_", "gho_"];
const GITHUB_TOKEN_MIN_BODY: usize = 30;
const PRIVATE_KEY_OPENING: &str = "-----BEGIN ";
const PRIVATE_KEY_CLOSING: &str = " PRIVATE KEY-----";
const SLACK_TOKEN_PREFIX: &str = "xox";
const SLACK_TOKEN_MIN_LENGTH: usize = 20;

/// The finding one added line produces. The value is quoted in the detail: a match is
/// already in the repository's history, so hiding it here protects nothing and leaves the
/// reader unable to find or rotate it.
struct Match {
    rule: &'static str,
    title: &'static str,
    severity: Severity,
    confidence: f32,
    detail: String,
}

pub fn added_line_findings(path: &RepoPath, base: Option<&str>, head: &str) -> Vec<NewFinding> {
    let base_lines = base.unwrap_or_default().lines().collect::<BTreeSet<_>>();
    let mut findings = Vec::new();
    for (index, line) in head.lines().enumerate() {
        if base_lines.contains(line) {
            continue;
        }
        let Some(found) = classify(line) else {
            continue;
        };
        let span = LineSpan {
            start: index as u32 + 1,
            end: index as u32 + 1,
        };
        let location = Location::File {
            path: path.clone(),
            span: Some(span),
        };
        findings.push(NewFinding {
            movement: None,
            fingerprint: Fingerprint::compute(&Components {
                analyzer: ANALYZER,
                rule: found.rule,
                location: &location,
                title: found.title,
                occurrence: 0,
            }),
            location,
            severity: found.severity,
            confidence: Confidence::new(found.confidence).expect("confidence is in range"),
            attribution: Attribution::Introduced,
            title: found.title.to_owned(),
            detail: found.detail,
        });
    }
    findings
}

fn classify(line: &str) -> Option<Match> {
    if let Some(key) = aws_access_key(line) {
        return Some(Match {
            rule: "aws-access-key",
            title: "AWS access key added to source",
            severity: Severity::Critical,
            confidence: 0.99,
            detail: format!(
                "{key} matches an AWS access key: {AWS_ACCESS_KEY_PREFIX} followed by {} \
                 uppercase letters or digits.",
                AWS_ACCESS_KEY_LENGTH - AWS_ACCESS_KEY_PREFIX.len()
            ),
        });
    }
    if let Some((prefix, token)) = github_token(line) {
        return Some(Match {
            rule: "github-token",
            title: "GitHub token added to source",
            severity: Severity::High,
            confidence: 0.98,
            detail: format!(
                "{token} matches a GitHub token: {prefix} followed by {GITHUB_TOKEN_MIN_BODY} \
                 or more letters or digits."
            ),
        });
    }
    if let Some(header) = private_key_header(line) {
        return Some(Match {
            rule: "private-key",
            title: "Private key added to source",
            severity: Severity::Critical,
            confidence: 0.99,
            detail: format!("{header} opens a private key block."),
        });
    }
    if let Some(token) = slack_token(line) {
        return Some(Match {
            rule: "slack-token",
            title: "Slack token added to source",
            severity: Severity::High,
            confidence: 0.97,
            detail: format!(
                "{token} matches a Slack token: {SLACK_TOKEN_PREFIX} and {SLACK_TOKEN_MIN_LENGTH} \
                 or more letters, digits, dashes or underscores."
            ),
        });
    }
    let (key, value) = assigned_secret_value(line)?;
    high_entropy(value).then(|| Match {
        rule: "high-entropy-secret",
        title: "High-entropy secret added to source",
        severity: Severity::High,
        confidence: 0.9,
        detail: format!(
            "{value} is assigned to {key}: {} characters, {} distinct, mixing at least three \
             of lowercase, uppercase, digits and symbols.",
            value.chars().count(),
            value.chars().collect::<BTreeSet<_>>().len()
        ),
    })
}

fn aws_access_key(line: &str) -> Option<&str> {
    line.as_bytes()
        .windows(AWS_ACCESS_KEY_LENGTH)
        .position(|candidate| {
            candidate.starts_with(AWS_ACCESS_KEY_PREFIX.as_bytes())
                && candidate[AWS_ACCESS_KEY_PREFIX.len()..]
                    .iter()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        .map(|start| &line[start..start + AWS_ACCESS_KEY_LENGTH])
}

fn github_token(line: &str) -> Option<(&'static str, &str)> {
    GITHUB_TOKEN_PREFIXES.iter().find_map(|prefix| {
        line.match_indices(prefix).find_map(|(start, _)| {
            let body = line[start + prefix.len()..]
                .bytes()
                .take_while(u8::is_ascii_alphanumeric)
                .count();
            (body >= GITHUB_TOKEN_MIN_BODY)
                .then(|| (*prefix, &line[start..start + prefix.len() + body]))
        })
    })
}

fn private_key_header(line: &str) -> Option<&str> {
    let end = line.find(PRIVATE_KEY_CLOSING)? + PRIVATE_KEY_CLOSING.len();
    let start = line[..end].rfind(PRIVATE_KEY_OPENING)?;
    Some(&line[start..end])
}

fn slack_token(line: &str) -> Option<&str> {
    line.match_indices(SLACK_TOKEN_PREFIX)
        .find_map(|(start, _)| {
            let token = line[start..].split_whitespace().next()?;
            (token.len() >= SLACK_TOKEN_MIN_LENGTH
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
            .then_some(token)
        })
}

/// The first key on the line that names a secret, and the quoted value after it.
fn assigned_secret_value(line: &str) -> Option<(&str, &str)> {
    let key = line
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '-'
        })
        .find(|part| {
            let lower = part.to_ascii_lowercase();
            [
                "secret",
                "token",
                "password",
                "passwd",
                "api_key",
                "apikey",
                "access_key",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
        })?;
    let start = line.find(key)? + key.len();
    let remainder =
        line[start..].trim_start_matches(|character: char| character != '"' && character != '\'');
    let quote = remainder.chars().next()?;
    let end = remainder[quote.len_utf8()..].find(quote)? + quote.len_utf8();
    Some((key, &remainder[quote.len_utf8()..end]))
}

fn high_entropy(value: &str) -> bool {
    if value.len() < 20
        || value.len() > 256
        || value.contains("example")
        || value.contains("changeme")
    {
        return false;
    }
    let classes = [
        value.chars().any(|c| c.is_ascii_lowercase()),
        value.chars().any(|c| c.is_ascii_uppercase()),
        value.chars().any(|c| c.is_ascii_digit()),
        value.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    classes >= 3 && value.chars().collect::<BTreeSet<_>>().len() >= 10
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path() -> RepoPath {
        RepoPath::new("src/config.rs").expect("path is valid")
    }

    #[test]
    fn reports_an_added_aws_access_key() {
        let findings = added_line_findings(
            &path(),
            Some("const name = \"app\";"),
            "const key = \"AKIA0123456789ABCDEF\";",
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[0].title, "AWS access key added to source");
        assert!(
            findings[0]
                .detail
                .starts_with("AKIA0123456789ABCDEF matches")
        );
    }

    #[test]
    fn ignores_a_secret_that_was_already_present() {
        let line = "const key = \"AKIA0123456789ABCDEF\";";
        assert!(added_line_findings(&path(), Some(line), line).is_empty());
    }

    #[test]
    fn quotes_exactly_the_matched_value() {
        let findings = added_line_findings(
            &path(),
            None,
            "PRIVATE_KEY=\"-----BEGIN RSA PRIVATE KEY-----\"\napi_secret=\"aB3!long-random-value-123\"",
        );
        assert_eq!(findings.len(), 2);
        assert!(
            findings[0]
                .detail
                .starts_with("-----BEGIN RSA PRIVATE KEY----- opens")
        );
        assert!(
            findings[1]
                .detail
                .starts_with("aB3!long-random-value-123 is assigned to api_secret:")
        );
    }

    #[test]
    fn reports_an_added_github_token() {
        let token = "ghp_abcdefghijklmnopqrstuvwxyz1234567890";
        let findings = added_line_findings(&path(), None, &format!("const token = \"{token}\";"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::High);
        assert!(findings[0].detail.starts_with(&format!("{token} matches")));
    }

    #[test]
    fn reports_an_added_slack_token() {
        let findings =
            added_line_findings(&path(), None, "SLACK_TOKEN=xoxb-1234567890-abcdefghijkl");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Slack token added to source");
        assert!(
            findings[0]
                .detail
                .starts_with("xoxb-1234567890-abcdefghijkl matches")
        );
    }

    #[test]
    fn ignores_a_benign_low_entropy_secret_named_value() {
        let findings = added_line_findings(&path(), None, "password=\"aaaaaaaaaaaaaaaaaaaa\"");
        assert!(findings.is_empty());
    }
}
