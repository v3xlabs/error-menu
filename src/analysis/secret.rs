use std::collections::BTreeSet;

use crate::analysis::finding::fingerprint::{Components, Fingerprint};
use crate::prelude::*;

pub const ANALYZER: &str = "secret-scan";

const AWS_ACCESS_KEY_PREFIX: &str = "AKIA";
const GITHUB_TOKEN_PREFIXES: [&str; 3] = ["ghp_", "github_pat_", "gho_"];
const SLACK_TOKEN_PREFIX: &str = "xox";

pub fn added_line_findings(path: &RepoPath, base: Option<&str>, head: &str) -> Vec<NewFinding> {
    let base_lines = base.unwrap_or_default().lines().collect::<BTreeSet<_>>();
    let mut findings = Vec::new();
    for (index, line) in head.lines().enumerate() {
        if base_lines.contains(line) {
            continue;
        }
        let Some((rule, title, severity, confidence)) = classify(line) else {
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
                rule,
                location: &location,
                title,
                occurrence: 0,
            }),
            location,
            severity,
            confidence: Confidence::new(confidence).expect("confidence is in range"),
            attribution: Attribution::Introduced,
            title: title.to_owned(),
            detail: "A credential-shaped value was added to this line. The value is not stored."
                .to_owned(),
        });
    }
    findings
}

fn classify(line: &str) -> Option<(&'static str, &'static str, Severity, f32)> {
    if line.as_bytes().windows(20).any(|candidate| {
        candidate.starts_with(AWS_ACCESS_KEY_PREFIX.as_bytes())
            && candidate[4..]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    }) {
        return Some((
            "aws-access-key",
            "AWS access key added to source",
            Severity::Critical,
            0.99,
        ));
    }
    if GITHUB_TOKEN_PREFIXES
        .iter()
        .any(|prefix| token_after_prefix_is_long_enough(line, prefix))
    {
        return Some((
            "github-token",
            "GitHub token added to source",
            Severity::High,
            0.98,
        ));
    }
    if line.contains("-----BEGIN ") && line.contains(" PRIVATE KEY-----") {
        return Some((
            "private-key",
            "Private key added to source",
            Severity::Critical,
            0.99,
        ));
    }
    if line.match_indices(SLACK_TOKEN_PREFIX).any(|(start, _)| {
        line[start..]
            .split_whitespace()
            .next()
            .is_some_and(|token| {
                token.len() >= 20
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            })
    }) {
        return Some((
            "slack-token",
            "Slack token added to source",
            Severity::High,
            0.97,
        ));
    }
    assigned_secret_value(line)
        .filter(|value| high_entropy(value))
        .map(|_| {
            (
                "high-entropy-secret",
                "High-entropy secret added to source",
                Severity::High,
                0.9,
            )
        })
}

fn assigned_secret_value(line: &str) -> Option<&str> {
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
    Some(&remainder[quote.len_utf8()..end])
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

fn token_after_prefix_is_long_enough(line: &str, prefix: &str) -> bool {
    line.match_indices(prefix).any(|(start, _)| {
        line[start + prefix.len()..]
            .bytes()
            .take_while(u8::is_ascii_alphanumeric)
            .count()
            >= 30
    })
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
        assert!(!findings[0].detail.contains("AKIA"));
    }

    #[test]
    fn ignores_a_secret_that_was_already_present() {
        let line = "const key = \"AKIA0123456789ABCDEF\";";
        assert!(added_line_findings(&path(), Some(line), line).is_empty());
    }

    #[test]
    fn reports_private_keys_and_entropy_secrets_without_values() {
        let findings = added_line_findings(
            &path(),
            None,
            "PRIVATE_KEY=\"-----BEGIN RSA PRIVATE KEY-----\"\napi_secret=\"aB3!long-random-value-123\"",
        );
        assert_eq!(findings.len(), 2);
        assert!(
            findings
                .iter()
                .all(|finding| !finding.detail.contains("long-random")
                    && !finding.detail.contains("BEGIN RSA"))
        );
    }

    #[test]
    fn reports_an_added_github_token() {
        let token = "ghp_abcdefghijklmnopqrstuvwxyz1234567890";
        let findings = added_line_findings(&path(), None, &format!("const token = \"{token}\";"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::High);
    }

    #[test]
    fn reports_an_added_slack_token() {
        let findings =
            added_line_findings(&path(), None, "SLACK_TOKEN=xoxb-1234567890-abcdefghijkl");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Slack token added to source");
    }

    #[test]
    fn ignores_a_benign_low_entropy_secret_named_value() {
        let findings = added_line_findings(&path(), None, "password=\"aaaaaaaaaaaaaaaaaaaa\"");
        assert!(findings.is_empty());
    }
}
