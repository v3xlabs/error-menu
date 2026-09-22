use std::collections::BTreeMap;

use serde::Deserialize;

use super::LockedPackage;
use crate::prelude::*;

pub(super) fn parse(
    text: &str,
) -> Result<Vec<LockedPackage>, Box<dyn std::error::Error + Send + Sync>> {
    let lockfile: Lockfile = serde_json::from_str(text)?;

    Ok(lockfile
        .nodes
        .into_iter()
        .filter(|(name, _)| *name != lockfile.root)
        .filter_map(|(name, node)| {
            let locked = node.locked?;
            Some(LockedPackage {
                name,
                // A flake input is pinned by revision, so the revision is its version.
                version: locked.rev.clone().unwrap_or_else(|| {
                    locked
                        .last_modified
                        .map(|at| at.to_string())
                        .unwrap_or_default()
                }),
                origin: origin_of(&locked),
                source: Some(describe(&locked)),
                integrity: locked.nar_hash,
            })
        })
        .collect())
}

fn describe(locked: &Locked) -> String {
    match (&locked.owner, &locked.repo, &locked.url) {
        (Some(owner), Some(repo), _) => format!("{}:{owner}/{repo}", locked.kind),
        (_, _, Some(url)) => format!("{}:{url}", locked.kind),
        _ => locked.kind.clone(),
    }
}

/// A flake input is a repository, so its origin is the repository it names. `describe`
/// stays for the detail sentence; this is the part a link can be built from.
fn origin_of(locked: &Locked) -> PackageOrigin {
    let host = match locked.kind.as_str() {
        "github" => "https://github.com",
        "gitlab" => "https://gitlab.com",
        "sourcehut" => "https://git.sr.ht",
        _ => {
            return match &locked.url {
                Some(url) => PackageOrigin::Remote { url: url.clone() },
                // A node with neither a host pair nor a url is a path input.
                None => PackageOrigin::Local,
            };
        }
    };

    match (&locked.owner, &locked.repo) {
        (Some(owner), Some(repo)) => PackageOrigin::Remote {
            url: format!("{host}/{owner}/{repo}"),
        },
        _ => PackageOrigin::Local,
    }
}

#[derive(Debug, Deserialize)]
struct Lockfile {
    #[serde(default)]
    nodes: BTreeMap<String, Node>,
    #[serde(default)]
    root: String,
}

#[derive(Debug, Deserialize)]
struct Node {
    locked: Option<Locked>,
}

#[derive(Debug, Deserialize)]
struct Locked {
    #[serde(rename = "type")]
    kind: String,
    owner: Option<String>,
    repo: Option<String>,
    url: Option<String>,
    rev: Option<String>,
    #[serde(rename = "narHash")]
    nar_hash: Option<String>,
    #[serde(rename = "lastModified")]
    last_modified: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCKFILE: &str = r#"{
      "nodes": {
        "root": { "inputs": { "nixpkgs": "nixpkgs" } },
        "nixpkgs": {
          "locked": {
            "type": "github",
            "owner": "NixOS",
            "repo": "nixpkgs",
            "rev": "ef34387ddd751e1ab8857adf4676492d32eb24ec",
            "narHash": "sha256-aaa",
            "lastModified": 1757721600
          }
        },
        "helper": {
          "locked": {
            "type": "git",
            "url": "https://example.invalid/helper",
            "rev": "0123456789abcdef0123456789abcdef01234567",
            "narHash": "sha256-bbb"
          }
        }
      },
      "root": "root",
      "version": 7
    }"#;

    #[test]
    fn reads_inputs_and_skips_the_root_node() {
        let packages = parse(LOCKFILE).expect("parses");
        assert_eq!(packages.len(), 2);

        let nixpkgs = packages
            .iter()
            .find(|package| package.name == "nixpkgs")
            .expect("nixpkgs is an input");
        assert_eq!(
            nixpkgs.origin,
            PackageOrigin::Remote {
                url: "https://github.com/NixOS/nixpkgs".to_owned()
            }
        );
        assert_eq!(nixpkgs.source.as_deref(), Some("github:NixOS/nixpkgs"));
        assert_eq!(nixpkgs.integrity.as_deref(), Some("sha256-aaa"));
        assert_eq!(nixpkgs.version, "ef34387ddd751e1ab8857adf4676492d32eb24ec");
    }

    #[test]
    fn a_git_input_points_at_its_own_url() {
        let packages = parse(LOCKFILE).expect("parses");
        let helper = packages
            .iter()
            .find(|package| package.name == "helper")
            .expect("helper is an input");

        assert_eq!(
            helper.origin,
            PackageOrigin::Remote {
                url: "https://example.invalid/helper".to_owned()
            }
        );
        assert_eq!(
            helper.source.as_deref(),
            Some("git:https://example.invalid/helper")
        );
    }

    #[test]
    fn rubbish_is_an_error() {
        assert!(parse("not json").is_err());
    }
}
