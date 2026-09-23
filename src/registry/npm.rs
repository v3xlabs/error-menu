use serde::Deserialize;

use super::{PackageFacts, ReadError, get};
use crate::prelude::*;

const REGISTRY: &str = "https://registry.npmjs.org";
const DOWNLOADS: &str = "https://api.npmjs.org/downloads/point/last-week";
const NPMX: &str = "https://npmx.dev/api/registry";

/// One first party read, then three that are allowed to fail. npmx.dev answers two things
/// nothing else answers cheaply, transitive install size and OSV counts, over routes it
/// does not document, so a route that stops answering leaves those fields empty rather
/// than failing the row.
pub(super) async fn read(
    client: &reqwest::Client,
    name: &str,
    version: &str,
) -> Result<PackageFacts, ReadError> {
    let release: Release = get(client, &format!("{REGISTRY}/{name}/{version}")).await?;
    let downloads = get::<Downloads>(client, &format!("{DOWNLOADS}/{name}"))
        .await
        .ok();
    let size = get::<InstallSize>(client, &format!("{NPMX}/install-size/{name}/v/{version}"))
        .await
        .ok();
    let advisories = get::<Vulnerabilities>(
        client,
        &format!("{NPMX}/vulnerabilities/{name}/v/{version}"),
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
        vulnerabilities: advisories.as_ref().map(|found| found.total_counts.total),
        vulnerabilities_high: advisories
            .as_ref()
            .map(|found| found.total_counts.critical + found.total_counts.high),
        repository: release.repository.and_then(repository_url),
        homepage: release.homepage,
        ..PackageFacts::known(Ecosystem::Npm, name, version)
    })
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
struct Vulnerabilities {
    #[serde(rename = "totalCounts")]
    total_counts: Counts,
}

#[derive(Debug, Deserialize)]
struct Counts {
    total: u32,
    critical: u32,
    high: u32,
}
