use serde::Deserialize;

use super::{Advisory, AdvisorySeverity, PackageFacts, ReadError, get};
use crate::prelude::*;
use crate::worker::resource::Resource;

const REGISTRY: &str = "https://registry.npmjs.org";
const DOWNLOADS: &str = "https://api.npmjs.org/downloads/point/last-week";
const NPMX: &str = "https://npmx.dev/api/registry";

/// npmx.dev answers what nothing else answers cheaply, transitive install size and OSV
/// advisories, over routes it does not document. The advisory read is required, because a
/// row without it would tell the audit that nothing is known against the version. Size and
/// downloads are allowed to fail and leave their fields empty.
pub(super) async fn read(
    client: &reqwest::Client,
    name: &str,
    version: &str,
) -> Result<PackageFacts, ReadError> {
    let release: Release = get(
        client,
        Resource::Npm,
        &format!("{REGISTRY}/{name}/{version}"),
    )
    .await?;
    let found: Vulnerabilities = get(
        client,
        Resource::Npm,
        &format!("{NPMX}/vulnerabilities/{name}/v/{version}"),
    )
    .await
    .map_err(|error| match error {
        ReadError::NotFound => ReadError::Unavailable(format!("npmx has no {name} {version}")),
        unavailable => unavailable,
    })?;
    if found.failed_queries > 0 {
        return Err(ReadError::Unavailable(format!(
            "npmx could not read {} advisory queries for {name} {version}",
            found.failed_queries
        )));
    }
    let downloads = get::<Downloads>(client, Resource::Npm, &format!("{DOWNLOADS}/{name}"))
        .await
        .ok();
    let size = get::<InstallSize>(
        client,
        Resource::Npm,
        &format!("{NPMX}/install-size/{name}/v/{version}"),
    )
    .await
    .ok();

    Ok(PackageFacts {
        size_bytes: release.dist.unpacked_size,
        checksum: release.dist.integrity,
        license: release.license,
        withdrawn: release.deprecated.and_then(deprecation),
        install_bytes: size.as_ref().map(|size| size.total_size),
        dependency_count: size.as_ref().map(|size| size.dependency_count),
        downloads_week: downloads.map(|downloads| downloads.downloads),
        repository: release.repository.and_then(repository_url),
        homepage: release.homepage,
        advisories: own_advisories(found, name),
        ..PackageFacts::known(Ecosystem::Npm, name, version)
    })
}

/// npmx walks the whole install tree. A dependency is its own coordinate with its own
/// facts, so only what is published against this package is kept here.
fn own_advisories(found: Vulnerabilities, name: &str) -> Vec<Advisory> {
    found
        .vulnerable_packages
        .into_iter()
        .filter(|package| package.depth == "root" && package.name == name)
        .flat_map(|package| package.vulnerabilities)
        .map(|found| Advisory {
            severity: AdvisorySeverity::from_rating(&found.severity),
            id: found.id,
            aliases: found.aliases,
            summary: found.summary,
            fixed_in: found.fixed_in,
        })
        .collect()
}

/// npm records a deprecation as the notice itself, and a few old packages record it as a
/// bare `true`.
fn deprecation(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(notice) => Some(notice),
        serde_json::Value::Bool(true) => Some("deprecated by its publisher".to_owned()),
        _ => None,
    }
}

fn repository_url(repository: Repository) -> Option<String> {
    let raw = match repository {
        Repository::Url(url) => url,
        Repository::Table { url } => url?,
    };

    Some(crate::vcs::browser_url(&raw))
}

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(default)]
    dist: Dist,
    license: Option<String>,
    homepage: Option<String>,
    repository: Option<Repository>,
    deprecated: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct Dist {
    #[serde(rename = "unpackedSize")]
    unpacked_size: Option<u64>,
    integrity: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Repository {
    Url(String),
    Table { url: Option<String> },
}

#[derive(Debug, Deserialize)]
struct Downloads {
    downloads: u64,
}

#[derive(Debug, Deserialize)]
struct InstallSize {
    #[serde(rename = "totalSize")]
    total_size: u64,
    #[serde(rename = "dependencyCount")]
    dependency_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vulnerabilities {
    vulnerable_packages: Vec<VulnerablePackage>,
    failed_queries: u32,
}

#[derive(Debug, Deserialize)]
struct VulnerablePackage {
    name: String,
    depth: String,
    vulnerabilities: Vec<NpmxAdvisory>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NpmxAdvisory {
    id: String,
    summary: Option<String>,
    severity: String,
    #[serde(default)]
    aliases: Vec<String>,
    fixed_in: Option<String>,
}
