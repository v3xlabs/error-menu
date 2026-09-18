use serde::Deserialize;
use serde_yaml::Value;

use super::LockedPackage;

/// pnpm frames its lockfile as an explicit YAML document: pnpm 12 writes `---` before the
/// first key and again after the last one, which a whole-stream read rejects as two
/// documents. Only the first document carries the lockfile, so that is what is read.
pub fn parse(text: &str) -> Result<Vec<LockedPackage>, serde_yaml::Error> {
    let Some(document) = serde_yaml::Deserializer::from_str(text).next() else {
        return Ok(Vec::new());
    };
    let document = Value::deserialize(document)?;
    let Some(packages) = document.get("packages").and_then(Value::as_mapping) else {
        return Ok(Vec::new());
    };

    Ok(packages
        .iter()
        .filter_map(|(key, value)| {
            let key = key.as_str()?;
            let (name, version) = package_coordinate(key)?;
            let resolution = value.get("resolution");
            let source = resolution
                .and_then(|resolution| resolution.get("tarball").or_else(|| resolution.get("repo")))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let integrity = resolution
                .and_then(|resolution| resolution.get("integrity"))
                .and_then(Value::as_str)
                .map(str::to_owned);

            Some(LockedPackage {
                name: name.to_owned(),
                version: version.to_owned(),
                registered: source.is_none(),
                source,
                integrity,
            })
        })
        .collect())
}

fn package_coordinate(key: &str) -> Option<(&str, &str)> {
    let separator = key.rfind('@')?;
    (separator > 0 && separator + 1 < key.len())
        .then_some((&key[..separator], &key[separator + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_scoped_registry_package() {
        let packages = parse(
            "packages:\n  '@solidjs/router@0.15.0':\n    resolution:\n      integrity: sha512-example\n",
        )
        .expect("parses");

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "@solidjs/router");
        assert_eq!(packages[0].version, "0.15.0");
        assert!(packages[0].registered);
    }

    #[test]
    fn recognises_a_tarball_as_non_registry_source() {
        let packages = parse(
            "packages:\n  'example@1.0.0':\n    resolution:\n      tarball: https://example.invalid/example.tgz\n",
        )
        .expect("parses");

        assert!(!packages[0].registered);
        assert_eq!(
            packages[0].source.as_deref(),
            Some("https://example.invalid/example.tgz")
        );
    }

    #[test]
    fn reads_a_lockfile_framed_as_an_explicit_document() {
        let packages = parse(
            "---\nlockfileVersion: '9.0'\n\npackages:\n\n  'pnpm@12.4.1':\n    resolution: {integrity: sha512-example}\n\n---\n",
        )
        .expect("parses");

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "pnpm");
        assert_eq!(packages[0].version, "12.4.1");
    }
}
