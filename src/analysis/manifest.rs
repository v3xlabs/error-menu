use std::collections::{BTreeMap, BTreeSet};

use crate::confidence::Confidence;
use crate::finding::fingerprint::{Components, Fingerprint};
use crate::finding::{Attribution, Ecosystem, Location, NewFinding, Severity, VersionMovement};
use crate::vcs::RepoPath;

pub const ANALYZER: &str = "manifest-delta";

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("{path} does not parse: {detail}")]
    Parse { path: String, detail: String },
    #[error("{path} is not a supported manifest")]
    Unsupported { path: String },
}

type Dependency = (String, String, Option<String>);

pub fn supports(path: &RepoPath) -> bool {
    matches!(path.as_str().rsplit('/').next(), Some("Cargo.toml" | "package.json"))
}

pub fn delta(path: &RepoPath, base: Option<&str>, head: Option<&str>) -> Result<Vec<NewFinding>, ManifestError> {
    let ecosystem = ecosystem(path)?;
    let before = base.map(|text| parse(path, ecosystem, text)).transpose()?.unwrap_or_default();
    let after = head.map(|text| parse(path, ecosystem, text)).transpose()?.unwrap_or_default();
    let before: BTreeMap<_, _> = before.into_iter().map(|item| (item.0.clone(), item)).collect();
    let after: BTreeMap<_, _> = after.into_iter().map(|item| (item.0.clone(), item)).collect();
    let mut findings = Vec::new();

    for name in before.keys().chain(after.keys()).cloned().collect::<BTreeSet<_>>() {
        let movement = match (before.get(&name), after.get(&name)) {
            (None, Some(_)) => VersionMovement::Added,
            (Some(_), None) => VersionMovement::Removed,
            (Some(old), Some(new)) if old.1 != new.1 || old.2 != new.2 => VersionMovement::Changed,
            _ => continue,
        };
        let dependency = after.get(&name).or_else(|| before.get(&name)).expect("dependency exists");
        let location = Location::Package {
            path: path.clone(), ecosystem, name: dependency.0.clone(), version: dependency.1.clone(),
        };
        let rule = match movement {
            VersionMovement::Added => "dependency-added",
            VersionMovement::Removed => "dependency-removed",
            _ => "dependency-changed",
        };
        let title = match movement {
            VersionMovement::Added => format!("{} dependency was added", dependency.0),
            VersionMovement::Removed => format!("{} dependency was removed", dependency.0),
            VersionMovement::Changed => format!("{} dependency constraint changed", dependency.0),
            _ => unreachable!(),
        };
        findings.push(NewFinding {
            movement: Some(movement),
            fingerprint: Fingerprint::compute(&Components { analyzer: ANALYZER, rule, location: &location, title: &title, occurrence: 0 }),
            location,
            severity: Severity::Info,
            confidence: Confidence::new(0.99).expect("confidence is in range"),
            attribution: Attribution::Introduced,
            title,
            detail: match movement {
                VersionMovement::Added => format!("{} was added with constraint {}{}.", dependency.0, dependency.1, source_suffix(dependency.2.as_deref())),
                VersionMovement::Removed => format!("{} was removed from the manifest.", dependency.0),
                VersionMovement::Changed => format!("{} changed from the previous manifest declaration.", dependency.0),
                _ => unreachable!(),
            },
        });
    }
    Ok(findings)
}

fn ecosystem(path: &RepoPath) -> Result<Ecosystem, ManifestError> {
    match path.as_str().rsplit('/').next() {
        Some("Cargo.toml") => Ok(Ecosystem::Cargo),
        Some("package.json") => Ok(Ecosystem::Npm),
        _ => Err(ManifestError::Unsupported { path: path.as_str().to_owned() }),
    }
}

fn parse(path: &RepoPath, ecosystem: Ecosystem, text: &str) -> Result<Vec<Dependency>, ManifestError> {
    match ecosystem {
        Ecosystem::Cargo => {
            let value: toml::Table = toml::from_str(text).map_err(|error| ManifestError::Parse { path: path.as_str().to_owned(), detail: error.to_string() })?;
            Ok(dependency_tables([value.get("dependencies"), value.get("dev-dependencies"), value.get("build-dependencies")]))
        }
        Ecosystem::Npm => {
            let value: serde_json::Value = serde_json::from_str(text).map_err(|error| ManifestError::Parse { path: path.as_str().to_owned(), detail: error.to_string() })?;
            Ok(json_dependency_tables(&value, ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"]))
        }
        Ecosystem::Nix => Err(ManifestError::Unsupported { path: path.as_str().to_owned() }),
    }
}

fn dependency_tables<'a>(tables: impl IntoIterator<Item = Option<&'a toml::Value>>) -> Vec<Dependency> {
    tables.into_iter().filter_map(|table| table.and_then(toml::Value::as_table)).flat_map(|table| table.iter().map(|(name, value)| {
        let (constraint, source) = match value {
            toml::Value::String(value) => (value.clone(), None),
            toml::Value::Table(table) => (table.get("version").and_then(toml::Value::as_str).unwrap_or("*").to_owned(), table.get("git").or_else(|| table.get("path")).or_else(|| table.get("registry")).and_then(toml::Value::as_str).map(str::to_owned)),
            _ => ("*".to_owned(), None),
        };
        (name.clone(), constraint, source)
    })).collect()
}

fn json_dependency_tables(value: &serde_json::Value, names: [&str; 4]) -> Vec<Dependency> {
    names.into_iter().filter_map(|name| value.get(name).and_then(serde_json::Value::as_object)).flat_map(|table| table.iter().map(|(name, value)| {
        let constraint = value.as_str().or_else(|| value.get("version").and_then(serde_json::Value::as_str)).unwrap_or("*").to_owned();
        let source = value.get("resolved").and_then(serde_json::Value::as_str).map(str::to_owned);
        (name.clone(), constraint, source)
    })).collect()
}

fn source_suffix(source: Option<&str>) -> String { source.map(|value| format!(" from {value}")).unwrap_or_default() }

#[cfg(test)]
mod tests {
    use super::*;
    fn path(name: &str) -> RepoPath { RepoPath::new(name).expect("valid path") }

    #[test]
    fn reports_cargo_add_remove_and_constraint_changes() {
        let findings = delta(&path("Cargo.toml"), Some("[package]\nname = \"old\"\nversion = \"0.1.0\"\n\n[dependencies]\nold = \"1.0\"\nkept = \"1.0\"\n"), Some("[package]\nname = \"new\"\nversion = \"0.1.0\"\n\n[dependencies]\nnew = \"2.0\"\nkept = \"2.0\"\n")).expect("parses");
        assert_eq!(findings.len(), 3);
        assert!(findings.iter().any(|finding| finding.movement == Some(VersionMovement::Added)));
        assert!(findings.iter().any(|finding| finding.movement == Some(VersionMovement::Removed)));
        assert!(findings.iter().any(|finding| finding.movement == Some(VersionMovement::Changed)));
    }

    #[test]
    fn reports_npm_source_changes_without_executing_manifest_content() {
        let findings = delta(&path("package.json"), Some(r#"{"dependencies":{"x":{"version":"1","resolved":"https://registry"}}}"#), Some(r#"{"dependencies":{"x":{"version":"1","resolved":"https://mirror"}}}"#)).expect("parses");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].movement, Some(VersionMovement::Changed));
    }

    #[test]
    fn rejects_malformed_manifests() {
        assert!(matches!(delta(&path("Cargo.toml"), None, Some("[dependencies")), Err(ManifestError::Parse { .. })));
    }

    #[test]
    fn rejects_unsupported_manifest_paths() {
        assert!(matches!(delta(&path("README.txt"), None, Some("anything")), Err(ManifestError::Unsupported { .. })));
    }

    #[test]
    fn reports_removed_dependencies_when_manifest_is_deleted() {
        let findings = delta(&path("Cargo.toml"), Some("[dependencies]\nserde = \"1.0\"\n"), None).expect("parses");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].movement, Some(VersionMovement::Removed));
        assert_eq!(findings[0].location, Location::Package {
            path: path("Cargo.toml"), ecosystem: Ecosystem::Cargo, name: "serde".to_owned(), version: "1.0".to_owned(),
        });
    }
}
