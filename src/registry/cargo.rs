use serde::Deserialize;

use super::{PackageFacts, ReadError, get};
use crate::prelude::*;

const CRATES: &str = "https://crates.io/api/v1/crates";
const DOCS: &str = "https://docs.rs/crate";

/// Three reads per version. The crate endpoint is called with `?include=` because without
/// it crates.io embeds every published version: 440990 bytes for serde against 952.
pub(super) async fn read(
    client: &reqwest::Client,
    name: &str,
    version: &str,
) -> Result<PackageFacts, ReadError> {
    let release: VersionResponse = get(client, &format!("{CRATES}/{name}/{version}")).await?;
    let published: CrateResponse = get(client, &format!("{CRATES}/{name}?include=")).await?;
    let documented = get::<DocsStatus>(client, &format!("{DOCS}/{name}/{version}/status.json"))
        .await
        .is_ok_and(|status| status.doc_status);

    Ok(PackageFacts {
        size_bytes: Some(release.version.crate_size),
        checksum: Some(release.version.checksum),
        license: release.version.license,
        // A yank carries no reason, so the fact is the whole sentence.
        withdrawn: release
            .version
            .yanked
            .then(|| "yanked from crates.io".to_owned()),
        downloads_week: None,
        documentation: published
            .krate
            .documentation
            .or_else(|| documented.then(|| format!("https://docs.rs/{name}/{version}"))),
        repository: published.krate.repository,
        homepage: published.krate.homepage,
        ..PackageFacts::known(Ecosystem::Cargo, name, version)
    })
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
    version: Version,
}

#[derive(Debug, Deserialize)]
struct Version {
    crate_size: u64,
    checksum: String,
    license: Option<String>,
    yanked: bool,
}

#[derive(Debug, Deserialize)]
struct CrateResponse {
    #[serde(rename = "crate")]
    krate: Crate,
}

#[derive(Debug, Deserialize)]
struct Crate {
    documentation: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DocsStatus {
    doc_status: bool,
}
