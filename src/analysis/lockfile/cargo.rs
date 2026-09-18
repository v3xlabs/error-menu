use serde::Deserialize;

use super::LockedPackage;

const REGISTRY_PREFIX: &str = "registry+";

pub(super) fn parse(
    text: &str,
) -> Result<Vec<LockedPackage>, Box<dyn std::error::Error + Send + Sync>> {
    let lockfile: Lockfile = toml::from_str(text)?;

    Ok(lockfile
        .package
        .into_iter()
        .map(|package| LockedPackage {
            // A package with no source is a workspace member, which is ordinary.
            registered: package
                .source
                .as_ref()
                .is_none_or(|source| source.starts_with(REGISTRY_PREFIX)),
            name: package.name,
            version: package.version,
            source: package.source,
            integrity: package.checksum,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct Lockfile {
    #[serde(default)]
    package: Vec<LockedEntry>,
}

#[derive(Debug, Deserialize)]
struct LockedEntry {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_registry_git_and_workspace_entries() {
        let text = "\
version = 4

[[package]]
name = \"serde\"
version = \"1.0.1\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"
checksum = \"aaa\"

[[package]]
name = \"helper\"
version = \"0.1.0\"
source = \"git+https://example.invalid/helper\"

[[package]]
name = \"error-menu\"
version = \"0.1.0\"
";
        let packages = parse(text).expect("parses");

        assert_eq!(packages.len(), 3);
        assert!(packages[0].registered);
        assert_eq!(packages[0].integrity.as_deref(), Some("aaa"));
        assert!(!packages[1].registered);
        assert!(packages[2].registered);
        assert_eq!(packages[2].source, None);
    }

    #[test]
    fn an_empty_lockfile_has_no_packages() {
        assert!(parse("version = 4\n").expect("parses").is_empty());
    }

    #[test]
    fn rubbish_is_an_error() {
        assert!(parse("not toml {{{").is_err());
    }
}
