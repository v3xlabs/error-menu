use std::collections::BTreeSet;

use crate::analysis::finding::fingerprint::{Components, Fingerprint};
use crate::prelude::*;

pub const ANALYZER: &str = "secret-scan";

const PRIVATE_KEY_OPENING: &str = "-----BEGIN ";
const PRIVATE_KEY_CLOSING: &str = " PRIVATE KEY-----";

/// A credential its issuer marks with a fixed prefix, so the prefix names what it is. The
/// prefix must start a word and the body must end one, so a prefix inside an identifier is
/// not read as a token.
struct TokenFormat {
    rule: &'static str,
    title: &'static str,
    kind: &'static str,
    severity: Severity,
    confidence: f32,
    prefixes: &'static [&'static str],
    alphabet: Alphabet,
    length: Length,
}

#[derive(Clone, Copy)]
enum Alphabet {
    UpperDigit,
    Letters,
    Alphanumeric,
    Hex,
    Word,
    WordDash,
    WordDashDot,
}

#[derive(Clone, Copy)]
enum Length {
    Exactly(usize),
    AtLeast(usize),
}

/// Prefixes and body lengths follow the formats the issuers publish, as the gitleaks rule set
/// records them. A body may be longer where an issuer has lengthened its tokens before.
const TOKEN_FORMATS: &[TokenFormat] = &[
    TokenFormat {
        rule: "aws-access-key",
        title: "AWS access key added to source",
        kind: "an AWS access key",
        severity: Severity::Critical,
        confidence: 0.99,
        prefixes: &["AKIA", "ASIA", "ABIA", "ACCA"],
        alphabet: Alphabet::UpperDigit,
        length: Length::Exactly(16),
    },
    TokenFormat {
        rule: "github-token",
        title: "GitHub token added to source",
        kind: "a GitHub token",
        severity: Severity::High,
        confidence: 0.98,
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        alphabet: Alphabet::Alphanumeric,
        length: Length::AtLeast(36),
    },
    TokenFormat {
        rule: "github-token",
        title: "GitHub token added to source",
        kind: "a fine-grained GitHub token",
        severity: Severity::High,
        confidence: 0.98,
        prefixes: &["github_pat_"],
        alphabet: Alphabet::Word,
        length: Length::AtLeast(82),
    },
    TokenFormat {
        rule: "gitlab-token",
        title: "GitLab token added to source",
        kind: "a GitLab personal access token",
        severity: Severity::High,
        confidence: 0.98,
        prefixes: &["glpat-"],
        // A routable token carries a dotted suffix that is part of the token.
        alphabet: Alphabet::WordDashDot,
        length: Length::AtLeast(20),
    },
    TokenFormat {
        rule: "slack-token",
        title: "Slack token added to source",
        kind: "a Slack token",
        severity: Severity::High,
        confidence: 0.97,
        prefixes: &[
            "xoxa-", "xoxb-", "xoxc-", "xoxd-", "xoxe-", "xoxo-", "xoxp-", "xoxr-", "xoxs-",
        ],
        alphabet: Alphabet::WordDash,
        length: Length::AtLeast(15),
    },
    TokenFormat {
        rule: "stripe-secret-key",
        title: "Stripe secret key added to source",
        kind: "a live Stripe secret key",
        severity: Severity::Critical,
        confidence: 0.98,
        prefixes: &["sk_live_", "rk_live_", "sk_prod_", "rk_prod_"],
        alphabet: Alphabet::Alphanumeric,
        length: Length::AtLeast(24),
    },
    TokenFormat {
        rule: "google-api-key",
        title: "Google API key added to source",
        kind: "a Google API key",
        severity: Severity::High,
        confidence: 0.95,
        prefixes: &["AIza"],
        alphabet: Alphabet::WordDash,
        length: Length::Exactly(35),
    },
    TokenFormat {
        rule: "openai-api-key",
        title: "OpenAI API key added to source",
        kind: "an OpenAI API key",
        severity: Severity::High,
        confidence: 0.97,
        prefixes: &["sk-proj-", "sk-svcacct-", "sk-admin-"],
        alphabet: Alphabet::WordDash,
        length: Length::AtLeast(64),
    },
    TokenFormat {
        rule: "anthropic-api-key",
        title: "Anthropic API key added to source",
        kind: "an Anthropic API key",
        severity: Severity::High,
        confidence: 0.98,
        prefixes: &["sk-ant-api03-", "sk-ant-admin01-"],
        alphabet: Alphabet::WordDash,
        length: Length::AtLeast(64),
    },
    TokenFormat {
        rule: "npm-token",
        title: "npm token added to source",
        kind: "an npm access token",
        severity: Severity::High,
        confidence: 0.97,
        prefixes: &["npm_"],
        alphabet: Alphabet::Alphanumeric,
        length: Length::Exactly(36),
    },
    TokenFormat {
        rule: "pypi-token",
        title: "PyPI token added to source",
        kind: "a PyPI upload token",
        severity: Severity::High,
        confidence: 0.98,
        // The macaroon header every PyPI token opens with, in base64.
        prefixes: &["pypi-AgEIcHlwaS5vcmc"],
        alphabet: Alphabet::WordDash,
        length: Length::AtLeast(50),
    },
    TokenFormat {
        rule: "huggingface-token",
        title: "Hugging Face token added to source",
        kind: "a Hugging Face access token",
        severity: Severity::High,
        confidence: 0.95,
        prefixes: &["hf_"],
        alphabet: Alphabet::Letters,
        length: Length::Exactly(34),
    },
    TokenFormat {
        rule: "shopify-token",
        title: "Shopify access token added to source",
        kind: "a Shopify access token",
        severity: Severity::High,
        confidence: 0.97,
        prefixes: &["shpat_", "shpca_", "shppa_", "shpss_"],
        alphabet: Alphabet::Hex,
        length: Length::Exactly(32),
    },
    TokenFormat {
        rule: "sendgrid-api-key",
        title: "SendGrid API key added to source",
        kind: "a SendGrid API key",
        severity: Severity::High,
        confidence: 0.95,
        prefixes: &["SG."],
        alphabet: Alphabet::WordDashDot,
        length: Length::AtLeast(66),
    },
];

impl TokenFormat {
    /// The first token of this format on the line, with the prefix it starts with.
    fn find<'line>(&self, line: &'line str) -> Option<(&'static str, &'line str)> {
        let bytes = line.as_bytes();
        self.prefixes.iter().find_map(|prefix| {
            line.match_indices(prefix).find_map(|(start, _)| {
                if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
                    return None;
                }
                let body_start = start + prefix.len();
                let body = bytes[body_start..]
                    .iter()
                    .take_while(|byte| self.alphabet.admits(**byte))
                    .count();
                let end = body_start + body;
                let ends_a_word = bytes
                    .get(end)
                    .is_none_or(|byte| !byte.is_ascii_alphanumeric());
                (ends_a_word && self.length.admits(body)).then(|| (*prefix, &line[start..end]))
            })
        })
    }
}

impl Alphabet {
    fn admits(self, byte: u8) -> bool {
        match self {
            Self::UpperDigit => byte.is_ascii_uppercase() || byte.is_ascii_digit(),
            Self::Letters => byte.is_ascii_alphabetic(),
            Self::Alphanumeric => byte.is_ascii_alphanumeric(),
            Self::Hex => byte.is_ascii_hexdigit(),
            Self::Word => byte.is_ascii_alphanumeric() || byte == b'_',
            Self::WordDash => byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-',
            Self::WordDashDot => {
                byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
            }
        }
    }

    fn described(self) -> &'static str {
        match self {
            Self::UpperDigit => "uppercase letters or digits",
            Self::Letters => "letters",
            Self::Alphanumeric => "letters or digits",
            Self::Hex => "hexadecimal digits",
            Self::Word => "letters, digits or underscores",
            Self::WordDash => "letters, digits, dashes or underscores",
            Self::WordDashDot => "letters, digits, dashes, underscores or dots",
        }
    }
}

impl Length {
    fn admits(self, length: usize) -> bool {
        match self {
            Self::Exactly(expected) => length == expected,
            Self::AtLeast(minimum) => length >= minimum,
        }
    }
}

impl std::fmt::Display for Length {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exactly(expected) => write!(formatter, "{expected}"),
            Self::AtLeast(minimum) => write!(formatter, "{minimum} or more"),
        }
    }
}

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
    if let Some((format, prefix, token)) = TOKEN_FORMATS.iter().find_map(|format| {
        format
            .find(line)
            .map(|(prefix, token)| (format, prefix, token))
    }) {
        return Some(Match {
            rule: format.rule,
            title: format.title,
            severity: format.severity,
            confidence: format.confidence,
            detail: format!(
                "{token} matches {}: {prefix} followed by {} {}.",
                format.kind,
                format.length,
                format.alphabet.described()
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

fn private_key_header(line: &str) -> Option<&str> {
    let end = line.find(PRIVATE_KEY_CLOSING)? + PRIVATE_KEY_CLOSING.len();
    let start = line[..end].rfind(PRIVATE_KEY_OPENING)?;
    Some(&line[start..end])
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

    /// Fixtures are assembled at run time, so this file never holds a token-shaped literal
    /// for a forge's push protection or for error.menu's own scan to report.
    fn token(prefix: &str, alphabet: &str, length: usize) -> String {
        format!(
            "{prefix}{}",
            alphabet.chars().cycle().take(length).collect::<String>()
        )
    }

    fn findings_for(value: &str) -> Vec<NewFinding> {
        added_line_findings(&path(), None, &format!("value = \"{value}\";"))
    }

    #[test]
    fn reports_each_token_format_and_quotes_the_token() {
        let fine_grained = format!(
            "{}_{}",
            token("github_pat_", "aZ9", 22),
            token("", "aZ9", 59)
        );
        let routable_gitlab = format!("{}.{}", token("glpat-", "aZ9-", 27), token("", "01a", 9));
        for (value, title) in [
            (token("ASIA", "Z7", 16), "AWS access key added to source"),
            (token("ghu_", "aZ9", 36), "GitHub token added to source"),
            (fine_grained, "GitHub token added to source"),
            (token("glpat-", "aZ9-", 20), "GitLab token added to source"),
            (routable_gitlab, "GitLab token added to source"),
            (
                token("xoxb-", "1234567890-", 24),
                "Slack token added to source",
            ),
            (
                token("sk_live_", "aZ9", 24),
                "Stripe secret key added to source",
            ),
            (token("AIza", "aZ9_-", 35), "Google API key added to source"),
            (
                token("sk-proj-", "aZ9_-", 100),
                "OpenAI API key added to source",
            ),
            (
                token("sk-ant-api03-", "aZ9_-", 95),
                "Anthropic API key added to source",
            ),
            (token("npm_", "aZ9", 36), "npm token added to source"),
            (
                token("pypi-AgEIcHlwaS5vcmc", "aZ9_-", 60),
                "PyPI token added to source",
            ),
            (token("hf_", "aZ", 34), "Hugging Face token added to source"),
            (
                token("shpat_", "a9f", 32),
                "Shopify access token added to source",
            ),
            (
                token("SG.", "aZ9_-.", 66),
                "SendGrid API key added to source",
            ),
        ] {
            let findings = findings_for(&value);
            assert_eq!(findings.len(), 1, "{value}");
            assert_eq!(findings[0].title, title, "{value}");
            assert!(
                findings[0].detail.starts_with(&format!("{value} matches")),
                "{}",
                findings[0].detail
            );
        }
    }

    #[test]
    fn ignores_a_prefix_inside_a_longer_word() {
        assert!(findings_for(&token("disk_live_", "aZ9", 30)).is_empty());
        assert!(findings_for(&token("bghp_", "aZ9", 36)).is_empty());
    }

    #[test]
    fn ignores_a_body_of_the_wrong_length_or_alphabet() {
        assert!(findings_for(&token("AIza", "aZ9_-", 34)).is_empty());
        assert!(findings_for(&token("shpat_", "a9g", 32)).is_empty());
        assert!(findings_for(&token("ghp_", "aZ9", 35)).is_empty());
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
    fn ignores_a_benign_low_entropy_secret_named_value() {
        let findings = added_line_findings(&path(), None, "password=\"aaaaaaaaaaaaaaaaaaaa\"");
        assert!(findings.is_empty());
    }
}
