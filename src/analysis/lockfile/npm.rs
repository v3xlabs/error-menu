use std::collections::BTreeMap;

use serde::Deserialize;

use super::LockedPackage;

const REGISTRY_PREFIX: &str = "https://registry.npmjs.org/";
const NESTING: &str = "node_modules/";

/// Reads the `packages` map of a version 2 or 3 `package-lock.json`. A version 1 lockfile
/// describes its tree in a different shape and is refused rather than read as empty: a
/// gate that cannot read a file must say so, not report that it found nothing.
pub(super) fn parse(
    text: &str,
) -> Result<Vec<LockedPackage>, Box<dyn std::error::Error + Send + Sync>> {
    let lockfile: Lockfile = serde_json::from_str(text)?;

    if lockfile.lockfile_version < 2 {
        return Err(format!(
            "package-lock.json version {} is not readable; only 2 and later are",
            lockfile.lockfile_version
        )
        .into());
    }

    Ok(lockfile
        .packages
        .into_iter()
        // The empty key is the project itself, and a link points at a workspace sibling
        // that the lockfile describes under its own key.
        .filter(|(location, entry)| !location.is_empty() && !entry.link)
        .filter_map(|(location, entry)| {
            let version = entry.version?;
            Some(LockedPackage {
                name: name_of(&location).to_owned(),
                version,
                registered: entry
                    .resolved
                    .as_ref()
                    .is_some_and(|resolved| resolved.starts_with(REGISTRY_PREFIX)),
                source: entry.resolved,
                integrity: entry.integrity,
            })
        })
        .collect())
}

/// Keys nest, so `node_modules/a/node_modules/b` is package `b` installed under `a`.
fn name_of(location: &str) -> &str {
    match location.rfind(NESTING) {
        Some(index) => &location[index + NESTING.len()..],
        None => location,
    }
}

#[derive(Debug, Deserialize)]
struct Lockfile {
    #[serde(rename = "lockfileVersion", default)]
    lockfile_version: u32,
    #[serde(default)]
    packages: BTreeMap<String, Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    version: Option<String>,
    resolved: Option<String>,
    integrity: Option<String>,
    #[serde(default)]
    link: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCKFILE: &str = r#"{
      "lockfileVersion": 3,
      "packages": {
        "": { "name": "site", "version": "1.0.0" },
        "node_modules/left-pad": {
          "version": "1.3.0",
          "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz",
          "integrity": "sha512-aaa"
        },
        "node_modules/patched/node_modules/left-pad": {
          "version": "1.3.1",
          "resolved": "https://example.invalid/left-pad.tgz",
          "integrity": "sha512-bbb"
        },
        "node_modules/workspace-link": { "resolved": "packages/inner", "link": true }
      }
    }"#;

    #[test]
    fn reads_nested_entries_and_skips_the_project_and_links() {
        let packages = parse(LOCKFILE).expect("parses");

        assert_eq!(packages.len(), 2);
        assert!(packages.iter().all(|package| package.name == "left-pad"));

        let registered = packages
            .iter()
            .find(|package| package.version == "1.3.0")
            .expect("the registry copy");
        assert!(registered.registered);
        assert_eq!(registered.integrity.as_deref(), Some("sha512-aaa"));

        let elsewhere = packages
            .iter()
            .find(|package| package.version == "1.3.1")
            .expect("the other copy");
        assert!(!elsewhere.registered);
    }

    #[test]
    fn a_version_one_lockfile_is_refused_rather_than_read_as_empty() {
        let text =
            r#"{ "lockfileVersion": 1, "dependencies": { "left-pad": { "version": "1.3.0" } } }"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn rubbish_is_an_error() {
        assert!(parse("not json").is_err());
    }
}
