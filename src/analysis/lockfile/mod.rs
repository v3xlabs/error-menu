mod cargo;
mod nix;
mod npm;
mod pnpm;

use std::collections::BTreeMap;

use crate::confidence::Confidence;
use crate::finding::fingerprint::{Components, Fingerprint};
use crate::finding::{Attribution, Ecosystem, Location, NewFinding, Severity, VersionMovement};
use crate::vcs::mirror::{FileChange, Mirror, MirrorError};
use crate::vcs::{CommitSha, RepoPath};

pub const ANALYZER: &str = "lockfile-delta";

const INTEGRITY_CHANGED: &str = "integrity-changed";
const SOURCE_CHANGED: &str = "source-changed";
const UNREGISTERED_ADDED: &str = "unregistered-dependency-added";
const PACKAGE_ADDED: &str = "package-added";
const PACKAGE_REMOVED: &str = "package-removed";
const PACKAGE_VERSION_CHANGED: &str = "package-version-changed";

#[derive(Debug, thiserror::Error)]
pub enum LockfileError {
    #[error("{path} at the {revision} revision does not parse: {source}")]
    Parse {
        path: String,
        revision: Revision,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("reading the repository: {0}")]
    Repository(#[from] MirrorError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    Base,
    Head,
}

impl std::fmt::Display for Revision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Base => "base",
            Self::Head => "head",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cargo,
    Npm,
    Pnpm,
    Nix,
}

impl Kind {
    pub fn of(path: &RepoPath) -> Option<Self> {
        match path.as_str().rsplit('/').next()? {
            "Cargo.lock" => Some(Self::Cargo),
            "package-lock.json" => Some(Self::Npm),
            "pnpm-lock.yaml" => Some(Self::Pnpm),
            "flake.lock" => Some(Self::Nix),
            _ => None,
        }
    }

    fn ecosystem(self) -> Ecosystem {
        match self {
            Self::Cargo => Ecosystem::Cargo,
            Self::Npm | Self::Pnpm => Ecosystem::Npm,
            Self::Nix => Ecosystem::Nix,
        }
    }

    pub fn parse(
        self,
        text: &str,
    ) -> Result<Vec<LockedPackage>, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Self::Cargo => cargo::parse(text),
            Self::Npm => npm::parse(text),
            Self::Pnpm => pnpm::parse(text).map_err(Into::into),
            Self::Nix => nix::parse(text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    /// Checksum, integrity hash or narHash, whichever the format carries.
    pub integrity: Option<String>,
    /// Whether this came from the ecosystem's ordinary registry. Each format decides for
    /// itself what ordinary means.
    pub registered: bool,
}

pub async fn delta_for_change(
    mirror: &Mirror,
    base: &CommitSha,
    head: &CommitSha,
) -> Result<Vec<NewFinding>, LockfileError> {
    let mut findings = Vec::new();

    for changed in mirror.changed_files(base, head).await? {
        let Some(kind) = Kind::of(&changed.path) else {
            continue;
        };
        if changed.change == FileChange::Deleted {
            continue;
        }

        let before = read(mirror, base, &changed.path).await?;
        let after = read(mirror, head, &changed.path).await?;
        let Some(after) = after else {
            continue;
        };

        findings.extend(delta(kind, &changed.path, before.as_deref(), &after)?);
    }

    Ok(findings)
}

async fn read(
    mirror: &Mirror,
    revision: &CommitSha,
    path: &RepoPath,
) -> Result<Option<String>, LockfileError> {
    Ok(mirror.file_at(revision, path).await?)
}

pub fn delta(
    kind: Kind,
    path: &RepoPath,
    base: Option<&str>,
    head: &str,
) -> Result<Vec<NewFinding>, LockfileError> {
    let base_packages = match base {
        Some(text) => parse(kind, path, Revision::Base, text)?,
        None => Vec::new(),
    };
    let head_packages = parse(kind, path, Revision::Head, head)?;

    let previous: BTreeMap<(&str, &str), &LockedPackage> = base_packages
        .iter()
        .map(|package| ((package.name.as_str(), package.version.as_str()), package))
        .collect();
    let current: BTreeMap<(&str, &str), &LockedPackage> = head_packages
        .iter()
        .map(|package| ((package.name.as_str(), package.version.as_str()), package))
        .collect();

    let mut occurrences: BTreeMap<(&str, String), u32> = BTreeMap::new();
    let mut findings = Vec::new();
    let mut appeared: BTreeMap<&str, Vec<&LockedPackage>> = BTreeMap::new();

    for package in &head_packages {
        let key = (package.name.as_str(), package.version.as_str());
        let (rule, severity, movement, detail) = match previous.get(&key) {
            // Source first: a changed source explains changed bytes, while changed bytes
            // under an unchanged source mean the same name and version now resolves to
            // something else.
            Some(before) if before.source != package.source => (
                SOURCE_CHANGED,
                Severity::High,
                None,
                format!(
                    "{} {} now resolves from {} instead of {}",
                    package.name,
                    package.version,
                    describe(package.source.as_deref()),
                    describe(before.source.as_deref())
                ),
            ),
            Some(before) if before.integrity != package.integrity => (
                INTEGRITY_CHANGED,
                Severity::Critical,
                None,
                format!(
                    "{} {} keeps its version and source but its integrity changed from {} to {}",
                    package.name,
                    package.version,
                    describe(before.integrity.as_deref()),
                    describe(package.integrity.as_deref())
                ),
            ),
            Some(_) => continue,
            // An unregistered arrival is reported on its own even when the same name also
            // lost a version, because where it now resolves from is the point, not the bump.
            None if !package.registered => (
                UNREGISTERED_ADDED,
                Severity::Medium,
                Some(VersionMovement::Added),
                format!(
                    "{} {} is new and resolves from {}, which no registry reviewed",
                    package.name,
                    package.version,
                    describe(package.source.as_deref())
                ),
            ),
            None => {
                appeared
                    .entry(package.name.as_str())
                    .or_default()
                    .push(package);
                continue;
            }
        };

        findings.push(finding(
            &mut occurrences,
            path,
            kind,
            Reported {
                rule,
                severity,
                package,
                movement,
                detail,
            },
        ));
    }

    let mut vanished: BTreeMap<&str, Vec<&LockedPackage>> = BTreeMap::new();
    for package in &base_packages {
        let key = (package.name.as_str(), package.version.as_str());
        if current.contains_key(&key) {
            continue;
        }

        vanished
            .entry(package.name.as_str())
            .or_default()
            .push(package);
    }

    // One name that both gained and lost a version was moved, not added and removed. Two
    // findings for one bump buries the changes that nobody asked for.
    for (name, arrivals) in appeared {
        let Some(departures) = vanished.remove(name) else {
            for package in arrivals {
                let detail = format!(
                    "{} {} is newly present in the lockfile",
                    package.name, package.version
                );
                findings.push(finding(
                    &mut occurrences,
                    path,
                    kind,
                    Reported {
                        rule: PACKAGE_ADDED,
                        severity: Severity::Info,
                        package,
                        movement: Some(VersionMovement::Added),
                        detail,
                    },
                ));
            }

            continue;
        };

        for package in &arrivals {
            let (direction, detail) = movement(&departures, package);
            findings.push(finding(
                &mut occurrences,
                path,
                kind,
                Reported {
                    rule: PACKAGE_VERSION_CHANGED,
                    severity: Severity::Info,
                    package,
                    movement: Some(direction),
                    detail,
                },
            ));
        }
    }

    for package in vanished.into_values().flatten() {
        let detail = format!(
            "{} {} is no longer present in the lockfile",
            package.name, package.version
        );
        findings.push(finding(
            &mut occurrences,
            path,
            kind,
            Reported {
                rule: PACKAGE_REMOVED,
                severity: Severity::Info,
                package,
                movement: Some(VersionMovement::Removed),
                detail,
            },
        ));
    }

    Ok(findings)
}
/// How a version moved. A lockfile can hold several copies of one package, so the wording
/// only claims a direction when exactly one version left and one arrived.
fn movement(departures: &[&LockedPackage], arrival: &LockedPackage) -> (VersionMovement, String) {
    let [departure] = departures else {
        let versions = departures
            .iter()
            .map(|package| package.version.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        return (
            VersionMovement::Changed,
            format!("{} {} replaces {}", arrival.name, arrival.version, versions),
        );
    };

    let (direction, wording) = match compare_versions(&departure.version, &arrival.version) {
        Some(std::cmp::Ordering::Less) => (VersionMovement::Upgraded, "was upgraded"),
        Some(std::cmp::Ordering::Greater) => (VersionMovement::Downgraded, "was downgraded"),
        _ => (VersionMovement::Changed, "changed"),
    };

    (
        direction,
        format!(
            "{} {wording} from {} to {}",
            arrival.name, departure.version, arrival.version
        ),
    )
}

/// Compares the numeric parts of two versions numerically and the rest as text, so that
/// 1.10.0 follows 1.9.0. Answers `None` unless both sides start with a number, because a
/// flake.lock pins a commit sha where one revision is neither above nor below another.
fn compare_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let left = segments(left)?;
    let right = segments(right)?;

    for (left, right) in left.iter().zip(right.iter()) {
        let order = match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(left), Ok(right)) => left.cmp(&right),
            _ => left.as_str().cmp(right.as_str()),
        };

        if order != std::cmp::Ordering::Equal {
            return Some(order);
        }
    }

    Some(left.len().cmp(&right.len()))
}

fn segments(value: &str) -> Option<Vec<String>> {
    let segments = value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    segments.first()?.parse::<u64>().ok()?;

    Some(segments)
}

struct Reported<'a> {
    rule: &'static str,
    severity: Severity,
    package: &'a LockedPackage,
    movement: Option<VersionMovement>,
    detail: String,
}

fn finding(
    occurrences: &mut BTreeMap<(&'static str, String), u32>,
    path: &RepoPath,
    kind: Kind,
    reported: Reported<'_>,
) -> NewFinding {
    let occurrence = occurrences
        .entry((reported.rule, reported.package.name.clone()))
        .or_insert(0);
    let location = Location::Package {
        path: path.clone(),
        ecosystem: kind.ecosystem(),
        name: reported.package.name.clone(),
        version: reported.package.version.clone(),
    };
    let title = title_for(
        reported.rule,
        &reported.package.name,
        &reported.package.version,
    );
    let built = NewFinding {
        movement: reported.movement,
        fingerprint: Fingerprint::compute(&Components {
            analyzer: ANALYZER,
            rule: reported.rule,
            location: &location,
            title: &title,
            occurrence: *occurrence,
        }),
        location,
        severity: reported.severity,
        confidence: Confidence::new(1.0).expect("one is in range"),
        attribution: Attribution::Introduced,
        title,
        detail: reported.detail,
    };

    *occurrence += 1;

    built
}

fn parse(
    kind: Kind,
    path: &RepoPath,
    revision: Revision,
    text: &str,
) -> Result<Vec<LockedPackage>, LockfileError> {
    kind.parse(text).map_err(|source| LockfileError::Parse {
        path: path.as_str().to_owned(),
        revision,
        source,
    })
}

fn title_for(rule: &str, name: &str, version: &str) -> String {
    match rule {
        INTEGRITY_CHANGED => format!("integrity of {name} changed without a version change"),
        SOURCE_CHANGED => format!("{name} changed where it resolves from"),
        UNREGISTERED_ADDED => format!("{name} was added from outside a registry"),
        PACKAGE_ADDED => format!("{name} {version} was added to the lockfile"),
        PACKAGE_REMOVED => format!("{name} {version} was removed from the lockfile"),
        PACKAGE_VERSION_CHANGED => format!("{name} moved to {version}"),
        _ => unreachable!("a lockfile finding must have a known rule"),
    }
}

fn describe(value: Option<&str>) -> String {
    value.unwrap_or("nothing").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> RepoPath {
        RepoPath::new("Cargo.lock").expect("a valid path")
    }

    fn cargo_lock(packages: &[(&str, &str, Option<&str>, Option<&str>)]) -> String {
        let registry = "registry+https://github.com/rust-lang/crates.io-index";
        let mut text = String::from("version = 4\n");
        for (name, version, source, checksum) in packages {
            text.push_str(&format!(
                "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n"
            ));
            if let Some(source) = source {
                let source = if *source == "registry" {
                    registry
                } else {
                    source
                };
                text.push_str(&format!("source = \"{source}\"\n"));
            }
            if let Some(checksum) = checksum {
                text.push_str(&format!("checksum = \"{checksum}\"\n"));
            }
        }
        text
    }

    fn run(base: &str, head: &str) -> Vec<NewFinding> {
        delta(Kind::Cargo, &path(), Some(base), head).expect("parses")
    }

    #[test]
    fn recognises_the_formats_it_can_read() {
        assert_eq!(
            Kind::of(&RepoPath::new("Cargo.lock").unwrap()),
            Some(Kind::Cargo)
        );
        assert_eq!(
            Kind::of(&RepoPath::new("crates/inner/Cargo.lock").unwrap()),
            Some(Kind::Cargo)
        );
        assert_eq!(
            Kind::of(&RepoPath::new("web/package-lock.json").unwrap()),
            Some(Kind::Npm)
        );
        assert_eq!(
            Kind::of(&RepoPath::new("flake.lock").unwrap()),
            Some(Kind::Nix)
        );
        assert_eq!(
            Kind::of(&RepoPath::new("pnpm-lock.yaml").unwrap()),
            Some(Kind::Pnpm)
        );
        assert_eq!(Kind::of(&RepoPath::new("src/main.rs").unwrap()), None);
    }

    #[test]
    fn an_unchanged_lockfile_reports_nothing() {
        let text = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        assert!(run(&text, &text).is_empty());
    }

    #[test]
    fn an_ordinary_version_bump_is_one_upgrade_and_not_two_findings() {
        let base = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[("serde", "1.0.2", Some("registry"), Some("bbb"))]);

        let findings = run(&base, &head);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
        assert_eq!(findings[0].title, "serde moved to 1.0.2");
        assert_eq!(findings[0].detail, "serde was upgraded from 1.0.1 to 1.0.2");
    }

    #[test]
    fn a_version_going_backwards_says_so() {
        let base = cargo_lock(&[("serde", "1.0.10", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[("serde", "1.0.9", Some("registry"), Some("bbb"))]);

        let findings = run(&base, &head);

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].detail,
            "serde was downgraded from 1.0.10 to 1.0.9"
        );
    }

    #[test]
    fn a_removed_package_is_informational() {
        let base = cargo_lock(&[
            ("serde", "1.0.1", Some("registry"), Some("aaa")),
            ("toml", "1.0.0", Some("registry"), Some("ccc")),
        ]);
        let head = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);

        let findings = run(&base, &head);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
        assert_eq!(
            findings[0].title,
            "toml 1.0.0 was removed from the lockfile"
        );
    }

    #[test]
    fn a_workspace_member_is_informational() {
        let base = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[
            ("serde", "1.0.1", Some("registry"), Some("aaa")),
            ("error-menu", "0.1.0", None, None),
        ]);

        let findings = run(&base, &head);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
        assert_eq!(
            findings[0].title,
            "error-menu 0.1.0 was added to the lockfile"
        );
    }

    #[test]
    fn a_swapped_integrity_at_the_same_version_is_critical() {
        let base = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("bbb"))]);

        let findings = run(&base, &head);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].detail.contains("aaa"));
        assert!(findings[0].detail.contains("bbb"));
    }

    #[test]
    fn a_changed_source_is_reported_instead_of_its_integrity() {
        let base = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[(
            "serde",
            "1.0.1",
            Some("git+https://example.invalid/serde"),
            Some("bbb"),
        )]);

        let findings = run(&base, &head);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::High);
    }

    #[test]
    fn a_new_unregistered_dependency_is_reported() {
        let base = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[
            ("serde", "1.0.1", Some("registry"), Some("aaa")),
            (
                "helper",
                "0.1.0",
                Some("git+https://example.invalid/helper"),
                None,
            ),
        ]);

        let findings = run(&base, &head);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Medium);
        assert_eq!(
            findings[0].location,
            Location::Package {
                path: path(),
                ecosystem: Ecosystem::Cargo,
                name: "helper".to_owned(),
                version: "0.1.0".to_owned(),
            }
        );
    }

    #[test]
    fn a_new_lockfile_reports_every_entry() {
        let head = cargo_lock(&[
            ("serde", "1.0.1", Some("registry"), Some("aaa")),
            (
                "helper",
                "0.1.0",
                Some("git+https://example.invalid/helper"),
                None,
            ),
        ]);

        let findings = delta(Kind::Cargo, &path(), None, &head).expect("parses");
        assert_eq!(findings.len(), 2);
        assert!(
            findings
                .iter()
                .any(|finding| finding.title == "serde 1.0.1 was added to the lockfile")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.title == "helper was added from outside a registry")
        );
    }

    #[test]
    fn two_lockfiles_in_one_repository_get_separate_fingerprints() {
        let base = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let head = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("bbb"))]);

        let here = delta(Kind::Cargo, &path(), Some(&base), &head).expect("parses");
        let there = delta(
            Kind::Cargo,
            &RepoPath::new("crates/inner/Cargo.lock").unwrap(),
            Some(&base),
            &head,
        )
        .expect("parses");

        assert_ne!(here[0].fingerprint, there[0].fingerprint);
    }

    #[test]
    fn a_malformed_lockfile_names_the_path_and_revision() {
        let good = cargo_lock(&[("serde", "1.0.1", Some("registry"), Some("aaa"))]);
        let error = delta(Kind::Cargo, &path(), Some(&good), "not toml {{{").expect_err("fails");
        let LockfileError::Parse { revision, path, .. } = error else {
            panic!("expected a parse failure");
        };
        assert_eq!(revision, Revision::Head);
        assert_eq!(path, "Cargo.lock");
    }
}
