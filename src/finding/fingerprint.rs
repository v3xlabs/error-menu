use serde::{Deserialize, Serialize};

use crate::finding::Location;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub version: u32,
    pub hash: String,
    pub canonical: String,
}

pub struct Components<'a> {
    pub analyzer: &'a str,
    /// Stable identifier from the analyzer. Empty when the analyzer has no rules, such as an
    /// LLM reviewer.
    pub rule: &'a str,
    pub location: &'a Location,
    pub title: &'a str,
    pub occurrence: u32,
}

impl Fingerprint {
    pub fn compute(components: &Components<'_>) -> Self {
        let canonical = [
            format!("finding-fingerprint:v{VERSION}"),
            components.analyzer.to_owned(),
            components.rule.to_owned(),
            encode_location(components.location),
            collapse_whitespace(components.title),
            components.occurrence.to_string(),
        ]
        .join("\0");

        Self {
            version: VERSION,
            hash: blake3::hash(canonical.as_bytes()).to_hex().to_string(),
            canonical,
        }
    }
}

/// Line spans are excluded because lines move. Package versions are excluded because a
/// version bump that leaves the problem in place is the same problem.
fn encode_location(location: &Location) -> String {
    match location {
        Location::File { path, span: _ } => format!("file\0{path}"),
        Location::Package {
            path,
            ecosystem,
            name,
            version: _,
        } => format!("package\0{path}\0{ecosystem:?}\0{name}"),
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::{Ecosystem, LineSpan};
    use crate::vcs::RepoPath;

    fn file(path: &str, start: u32) -> Location {
        Location::File {
            path: RepoPath::new(path).expect("valid path"),
            span: Some(LineSpan {
                start,
                end: start + 2,
            }),
        }
    }

    fn components<'a>(location: &'a Location, title: &'a str) -> Components<'a> {
        Components {
            analyzer: "secret-scan",
            rule: "aws-access-key",
            location,
            title,
            occurrence: 0,
        }
    }

    #[test]
    fn survives_line_movement() {
        let near_top = file("src/main.rs", 12);
        let far_down = file("src/main.rs", 412);
        assert_eq!(
            Fingerprint::compute(&components(&near_top, "hardcoded key")),
            Fingerprint::compute(&components(&far_down, "hardcoded key"))
        );
    }

    #[test]
    fn survives_a_package_version_bump() {
        let before = Location::Package {
            path: RepoPath::new("Cargo.lock").expect("valid path"),
            ecosystem: Ecosystem::Cargo,
            name: "serde".to_owned(),
            version: "1.0.1".to_owned(),
        };
        let after = Location::Package {
            path: RepoPath::new("Cargo.lock").expect("valid path"),
            ecosystem: Ecosystem::Cargo,
            name: "serde".to_owned(),
            version: "1.0.2".to_owned(),
        };
        assert_eq!(
            Fingerprint::compute(&components(&before, "advisory")),
            Fingerprint::compute(&components(&after, "advisory"))
        );
    }

    #[test]
    fn separates_repeat_occurrences_in_one_file() {
        let location = file("src/main.rs", 12);
        let first = Fingerprint::compute(&components(&location, "hardcoded key"));
        let second = Fingerprint::compute(&Components {
            occurrence: 1,
            ..components(&location, "hardcoded key")
        });
        assert_ne!(first, second);
    }

    #[test]
    fn separates_different_files_and_rules() {
        let here = file("src/main.rs", 12);
        let there = file("src/other.rs", 12);
        assert_ne!(
            Fingerprint::compute(&components(&here, "hardcoded key")),
            Fingerprint::compute(&components(&there, "hardcoded key"))
        );

        let other_rule = Components {
            rule: "gcp-service-account",
            ..components(&here, "hardcoded key")
        };
        assert_ne!(
            Fingerprint::compute(&components(&here, "hardcoded key")),
            Fingerprint::compute(&other_rule)
        );
    }

    #[test]
    fn ignores_reformatted_whitespace_in_the_title() {
        let location = file("src/main.rs", 12);
        assert_eq!(
            Fingerprint::compute(&components(&location, "hardcoded key")),
            Fingerprint::compute(&components(&location, "  hardcoded\n  key  "))
        );
    }

    /// Known limit of v1: a reworded message and a renamed file both break identity, so
    /// analyzers should emit a stable `rule`; the algorithm is versioned so this can be fixed
    /// without losing history.
    #[test]
    fn rewording_and_renaming_break_identity() {
        let location = file("src/main.rs", 12);
        assert_ne!(
            Fingerprint::compute(&components(&location, "hardcoded key")),
            Fingerprint::compute(&components(&location, "hard coded credential"))
        );

        let renamed = file("src/entry.rs", 12);
        assert_ne!(
            Fingerprint::compute(&components(&location, "hardcoded key")),
            Fingerprint::compute(&components(&renamed, "hardcoded key"))
        );
    }
}
