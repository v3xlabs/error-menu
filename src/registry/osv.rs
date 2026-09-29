//! Advisories for one crate version from OSV, which carries both the GitHub advisory
//! database and RustSec.

use serde::{Deserialize, Serialize};

use super::{Advisory, AdvisorySeverity, ReadError, post};
use crate::analysis::lockfile::compare_versions;
use crate::worker::resource::Resource;

const QUERY: &str = "https://api.osv.dev/v1/query";
const CRATES_IO: &str = "crates.io";

pub(super) async fn crate_advisories(
    client: &reqwest::Client,
    name: &str,
    version: &str,
) -> Result<Vec<Advisory>, ReadError> {
    let found: Found = post(
        client,
        Resource::Osv,
        QUERY,
        &Query {
            package: QueryPackage {
                name,
                ecosystem: CRATES_IO,
            },
            version,
        },
    )
    .await
    // OSV answers an unknown package with an empty list, so a 404 is the API, not the crate.
    .map_err(|error| match error {
        ReadError::NotFound => ReadError::Unavailable(format!("{QUERY} answered 404")),
        unavailable => unavailable,
    })?;

    Ok(merge(found.vulns, name, version))
}

/// GitHub and RustSec each publish most crate advisories, under ids that name each other
/// as aliases. The GitHub record carries the rating, so it is the one kept.
fn merge(mut records: Vec<Record>, name: &str, version: &str) -> Vec<Advisory> {
    records.sort_by_key(|record| !record.id.starts_with("GHSA-"));

    let mut kept: Vec<Advisory> = Vec::new();
    for record in records {
        let duplicate = kept.iter().any(|advisory| {
            advisory.aliases.contains(&record.id) || record.aliases.contains(&advisory.id)
        });
        if duplicate {
            continue;
        }

        let affected = record
            .affected
            .iter()
            .filter(|affected| affected.package.name == name);
        let informational = affected
            .clone()
            .any(|affected| affected.database_specific.informational.is_some());
        let severity = match record.database_specific.severity.as_deref() {
            Some(rating) => AdvisorySeverity::from_rating(rating),
            None if informational => AdvisorySeverity::Informational,
            None => AdvisorySeverity::Unrated,
        };
        let fixed_in = affected
            .flat_map(|affected| &affected.ranges)
            .flat_map(|range| &range.events)
            .filter_map(|event| event.fixed.as_deref())
            .filter(|fixed| compare_versions(version, fixed) == Some(std::cmp::Ordering::Less))
            .min_by(|left, right| {
                compare_versions(left, right).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(str::to_owned);

        kept.push(Advisory {
            id: record.id,
            aliases: record.aliases,
            severity,
            summary: record.summary,
            fixed_in,
        });
    }

    kept
}

#[derive(Serialize)]
struct Query<'a> {
    package: QueryPackage<'a>,
    version: &'a str,
}

#[derive(Serialize)]
struct QueryPackage<'a> {
    name: &'a str,
    ecosystem: &'a str,
}

#[derive(Debug, Deserialize)]
struct Found {
    #[serde(default)]
    vulns: Vec<Record>,
}

#[derive(Debug, Deserialize)]
struct Record {
    id: String,
    #[serde(default, deserialize_with = "nullable")]
    aliases: Vec<String>,
    summary: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    database_specific: RecordSpecific,
    #[serde(default)]
    affected: Vec<Affected>,
}

#[derive(Debug, Default, Deserialize)]
struct RecordSpecific {
    severity: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Affected {
    package: AffectedPackage,
    #[serde(default)]
    ranges: Vec<Range>,
    #[serde(default, deserialize_with = "nullable")]
    database_specific: AffectedSpecific,
}

#[derive(Debug, Deserialize)]
struct AffectedPackage {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct AffectedSpecific {
    informational: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Range {
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Debug, Deserialize)]
struct Event {
    fixed: Option<String>,
}

/// OSV writes `null` where a field has nothing, as well as leaving it out.
fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
