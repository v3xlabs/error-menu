use serde::Deserialize;

use super::LockedPackage;
use crate::prelude::*;

const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
const REGISTRY_PREFIXES: [&str; 2] = ["registry+", "sparse+"];
const GIT_PREFIX: &str = "git+";

pub(super) fn parse(
    text: &str,
) -> Result<Vec<LockedPackage>, Box<dyn std::error::Error + Send + Sync>> {
    let lockfile: Lockfile = toml::from_str(text)?;

    Ok(lockfile
        .package
        .into_iter()
        .map(|package| LockedPackage {
            origin: origin_of(package.source.as_deref()),
            name: package.name,
            version: package.version,
            source: package.source,
            integrity: package.checksum,
        })
        .collect())
}

fn origin_of(source: Option<&str>) -> PackageOrigin {
    // A package with no source is a workspace member, which is ordinary.
    let Some(source) = source else {
        return PackageOrigin::Local;
    };

    if source == CRATES_IO {
        return PackageOrigin::PublicRegistry;
    }

    if let Some(url) = source.strip_prefix(GIT_PREFIX) {
        // A git source carries its branch as a query and its revision as a fragment, and
        // neither belongs in a link to the repository.
        let url = url.split(['?', '#']).next().unwrap_or(url);

        return PackageOrigin::Remote {
            url: url.to_owned(),
        };
    }

    match REGISTRY_PREFIXES
        .iter()
        .find_map(|prefix| source.strip_prefix(prefix))
    {
        Some(url) => PackageOrigin::Registry {
            url: url.to_owned(),
        },
        None => PackageOrigin::Remote {
            url: source.to_owned(),
        },
    }
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
source = \"git+https://example.invalid/helper?branch=main#0f1e2d\"

[[package]]
name = \"error-menu\"
version = \"0.1.0\"

[[package]]
name = \"inner\"
version = \"2.0.0\"
source = \"registry+https://packages.example.invalid/index\"

[[package]]
name = \"sparse-inner\"
version = \"1.0.0\"
source = \"sparse+https://sparse.example.invalid/index/\"
";
        let packages = parse(text).expect("parses");

        assert_eq!(packages.len(), 5);
        assert_eq!(packages[0].origin, PackageOrigin::PublicRegistry);
        assert_eq!(packages[0].integrity.as_deref(), Some("aaa"));
        assert_eq!(
            packages[1].origin,
            PackageOrigin::Remote {
                url: "https://example.invalid/helper".to_owned()
            }
        );
        assert_eq!(packages[2].origin, PackageOrigin::Local);
        assert_eq!(packages[2].source, None);
        assert_eq!(
            packages[3].origin,
            PackageOrigin::Registry {
                url: "https://packages.example.invalid/index".to_owned()
            }
        );
        assert_eq!(
            packages[4].origin,
            PackageOrigin::Registry {
                url: "https://sparse.example.invalid/index/".to_owned()
            }
        );
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
