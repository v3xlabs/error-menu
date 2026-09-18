use std::collections::BTreeMap;

use crate::confidence::Confidence;
use crate::finding::fingerprint::{Components, Fingerprint};
use crate::finding::{Attribution, Location, NewFinding, Severity};
use crate::vcs::RepoPath;

pub const ANALYZER: &str = "repository-controls";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Attributes,
    Codeowners,
    Gitmodules,
    PackageManager,
    RepositoryAutomation,
}

impl ControlKind {
    fn rule(self) -> &'static str {
        match self {
            Self::Attributes => "attributes",
            Self::Codeowners => "codeowners",
            Self::Gitmodules => "gitmodules",
            Self::PackageManager => "package-manager",
            Self::RepositoryAutomation => "repository-automation",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Attributes => ".gitattributes",
            Self::Codeowners => "CODEOWNERS",
            Self::Gitmodules => ".gitmodules",
            Self::PackageManager => "package-manager control file",
            Self::RepositoryAutomation => "repository automation control file",
        }
    }

    fn severity(self) -> Severity {
        match self {
            Self::Attributes | Self::Codeowners | Self::Gitmodules => Severity::High,
            Self::PackageManager | Self::RepositoryAutomation => Severity::Medium,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub name: String,
    pub path: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmoduleChange {
    pub before: Option<Submodule>,
    pub after: Option<Submodule>,
}

pub struct ControlReport {
    pub findings: Vec<NewFinding>,
    pub submodules: Vec<SubmoduleChange>,
}

pub fn report(path: &RepoPath, base: Option<&str>, head: Option<&str>) -> ControlReport {
    let Some(kind) = kind(path) else {
        return ControlReport {
            findings: Vec::new(),
            submodules: Vec::new(),
        };
    };
    if base == head {
        return ControlReport {
            findings: Vec::new(),
            submodules: Vec::new(),
        };
    }

    let submodules = (kind == ControlKind::Gitmodules)
        .then(|| submodule_changes(base.unwrap_or_default(), head.unwrap_or_default()))
        .unwrap_or_default();

    ControlReport {
        findings: vec![finding(path, kind)],
        submodules,
    }
}

pub fn kind(path: &RepoPath) -> Option<ControlKind> {
    let value = path.as_str();
    let name = value.rsplit('/').next().unwrap_or(value);

    if name == "CODEOWNERS" {
        return Some(ControlKind::Codeowners);
    }
    if name == ".gitattributes" {
        return Some(ControlKind::Attributes);
    }
    if name == ".gitmodules" {
        return Some(ControlKind::Gitmodules);
    }
    if matches!(
        value,
        ".github/dependabot.yml"
            | ".github/dependabot.yaml"
            | ".github/renovate.json"
            | ".github/renovate.json5"
            | ".renovaterc"
            | ".renovaterc.json"
    ) {
        return Some(ControlKind::RepositoryAutomation);
    }
    if matches!(
        value,
        ".npmrc"
            | ".yarnrc"
            | ".yarnrc.yml"
            | ".pnpmfile.cjs"
            | "pnpm-workspace.yaml"
            | ".cargo/config"
            | ".cargo/config.toml"
            | "rust-toolchain"
            | "rust-toolchain.toml"
    ) {
        return Some(ControlKind::PackageManager);
    }

    None
}

pub fn submodules(text: &str) -> Vec<Submodule> {
    let mut parsed = BTreeMap::new();
    let mut current = None::<ParsedSubmodule>;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = section_name(trimmed) {
            if let Some(submodule) = current.take().and_then(ParsedSubmodule::complete) {
                parsed.insert(submodule.name.clone(), submodule);
            }
            current = Some(ParsedSubmodule {
                name: name.to_owned(),
                path: None,
                url: None,
            });
            continue;
        }
        let Some(current) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        match key.trim() {
            "path" => current.path = Some(value.trim().to_owned()),
            "url" => current.url = Some(value.trim().to_owned()),
            _ => {}
        }
    }

    if let Some(submodule) = current.and_then(ParsedSubmodule::complete) {
        parsed.insert(submodule.name.clone(), submodule);
    }

    parsed.into_values().collect()
}

fn submodule_changes(base: &str, head: &str) -> Vec<SubmoduleChange> {
    let base = submodules(base)
        .into_iter()
        .map(|submodule| (submodule.name.clone(), submodule))
        .collect::<BTreeMap<_, _>>();
    let head = submodules(head)
        .into_iter()
        .map(|submodule| (submodule.name.clone(), submodule))
        .collect::<BTreeMap<_, _>>();
    let names = base.keys().chain(head.keys()).collect::<std::collections::BTreeSet<_>>();

    names
        .into_iter()
        .filter_map(|name| {
            let before = base.get(name).cloned();
            let after = head.get(name).cloned();
            (before != after).then_some(SubmoduleChange { before, after })
        })
        .collect()
}

struct ParsedSubmodule {
    name: String,
    path: Option<String>,
    url: Option<String>,
}

impl ParsedSubmodule {
    fn complete(self) -> Option<Submodule> {
        Some(Submodule {
            name: self.name,
            path: self.path?,
            url: self.url?,
        })
    }
}

fn section_name(value: &str) -> Option<&str> {
    let value = value.strip_prefix("[submodule \"")?;
    let value = value.strip_suffix("\"]")?;

    (!value.is_empty()).then_some(value)
}

fn finding(path: &RepoPath, kind: ControlKind) -> NewFinding {
    let location = Location::File {
        path: path.clone(),
        span: None,
    };
    let title = format!("{} changed", kind.label());

    NewFinding {
        movement: None,
        fingerprint: Fingerprint::compute(&Components {
            analyzer: ANALYZER,
            rule: kind.rule(),
            location: &location,
            title: &title,
            occurrence: 0,
        }),
        location,
        severity: kind.severity(),
        confidence: Confidence::new(1.0).expect("one is in range"),
        attribution: Attribution::Introduced,
        detail: format!("{} was changed.", path.as_str()),
        title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> RepoPath {
        RepoPath::new(value).expect("path is valid")
    }

    #[test]
    fn identifies_repository_and_package_manager_control_files() {
        assert_eq!(kind(&path(".github/CODEOWNERS")), Some(ControlKind::Codeowners));
        assert_eq!(kind(&path(".gitattributes")), Some(ControlKind::Attributes));
        assert_eq!(kind(&path(".gitmodules")), Some(ControlKind::Gitmodules));
        assert_eq!(kind(&path(".npmrc")), Some(ControlKind::PackageManager));
        assert_eq!(kind(&path("src/main.rs")), None);
    }

    #[test]
    fn reports_each_changed_control_file_once() {
        let report = report(&path(".github/CODEOWNERS"), Some("/src @team"), Some("/src @security"));

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].title, "CODEOWNERS changed");
        assert_eq!(report.findings[0].severity, Severity::High);
    }

    #[test]
    fn keeps_submodule_url_and_path_as_structured_facts() {
        let base = "[submodule \"docs\"]\n  path = docs\n  url = https://example.test/docs.git\n";
        let head = "[submodule \"docs\"]\n  path = vendor/docs\n  url = https://example.test/docs-next.git\n";

        let report = report(&path(".gitmodules"), Some(base), Some(head));

        assert_eq!(report.submodules.len(), 1);
        assert_eq!(
            report.submodules[0],
            SubmoduleChange {
                before: Some(Submodule {
                    name: "docs".to_owned(),
                    path: "docs".to_owned(),
                    url: "https://example.test/docs.git".to_owned(),
                }),
                after: Some(Submodule {
                    name: "docs".to_owned(),
                    path: "vendor/docs".to_owned(),
                    url: "https://example.test/docs-next.git".to_owned(),
                }),
            }
        );
    }

    #[test]
    fn does_not_report_an_unchanged_control_file() {
        let content = "registry=https://registry.example.test\n";

        assert!(report(&path(".npmrc"), Some(content), Some(content)).findings.is_empty());
    }
}
