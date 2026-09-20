use std::collections::BTreeMap;

use crate::prelude::*;

#[derive(Debug, Default, PartialEq)]
pub struct Comparison<'a> {
    pub new: Vec<&'a Finding>,
    pub existing: Vec<&'a Finding>,
    pub resolved: Vec<&'a Finding>,
}

/// `new` and `existing` are drawn from `current`; `resolved` from `previous`, because a
/// resolved finding exists only in the run that last saw it.
pub fn compare<'a>(previous: &'a [Finding], current: &'a [Finding]) -> Comparison<'a> {
    let previous_by_hash = index(previous);
    let current_by_hash = index(current);

    let mut comparison = Comparison::default();

    for (hash, finding) in &current_by_hash {
        if previous_by_hash.contains_key(hash) {
            comparison.existing.push(finding);
        } else {
            comparison.new.push(finding);
        }
    }

    for (hash, finding) in &previous_by_hash {
        if !current_by_hash.contains_key(hash) {
            comparison.resolved.push(finding);
        }
    }

    comparison
}

fn index(findings: &[Finding]) -> BTreeMap<&str, &Finding> {
    findings
        .iter()
        .map(|finding| (finding.fingerprint.hash.as_str(), finding))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::finding::fingerprint::{Components, Fingerprint};
    use crate::id::IdGenerator;

    fn finding(generator: &IdGenerator, path: &str, title: &str) -> Finding {
        let location = Location::File {
            path: RepoPath::new(path).expect("valid path"),
            span: None,
        };
        Finding {
            movement: None,
            id: generator.next(),
            run_id: generator.next(),
            issue_id: generator.next(),
            fingerprint: Fingerprint::compute(&Components {
                analyzer: "secret-scan",
                rule: "aws-access-key",
                location: &location,
                title,
                occurrence: 0,
            }),
            location,
            severity: Severity::High,
            confidence: Confidence::new(0.9).expect("in range"),
            attribution: Attribution::Introduced,
            title: title.to_owned(),
            detail: String::new(),
        }
    }

    fn hashes(findings: &[&Finding]) -> Vec<String> {
        findings
            .iter()
            .map(|finding| finding.fingerprint.hash.clone())
            .collect()
    }

    #[test]
    fn splits_into_new_existing_and_resolved() {
        let generator = IdGenerator::new(0);
        let kept = finding(&generator, "src/a.rs", "hardcoded key");
        let gone = finding(&generator, "src/b.rs", "hardcoded key");
        let added = finding(&generator, "src/c.rs", "hardcoded key");

        let previous = vec![kept.clone(), gone.clone()];
        let current = vec![kept.clone(), added.clone()];
        let comparison = compare(&previous, &current);

        assert_eq!(hashes(&comparison.existing), hashes(&[&kept]));
        assert_eq!(hashes(&comparison.new), hashes(&[&added]));
        assert_eq!(hashes(&comparison.resolved), hashes(&[&gone]));
    }

    #[test]
    fn a_finding_that_only_moved_lines_is_not_new() {
        let generator = IdGenerator::new(0);
        let before = finding(&generator, "src/a.rs", "hardcoded key");
        let mut after = finding(&generator, "src/a.rs", "hardcoded key");
        after.id = Id::from_raw(before.id.raw() + 1);
        after.location = Location::File {
            path: RepoPath::new("src/a.rs").expect("valid path"),
            span: Some(LineSpan {
                start: 400,
                end: 402,
            }),
        };

        let previous = [before];
        let current = [after];
        let comparison = compare(&previous, &current);
        assert_eq!(comparison.existing.len(), 1);
        assert!(comparison.new.is_empty());
        assert!(comparison.resolved.is_empty());
    }

    #[test]
    fn an_empty_baseline_makes_everything_new() {
        let generator = IdGenerator::new(0);
        let only = finding(&generator, "src/a.rs", "hardcoded key");
        let comparison = compare(&[], std::slice::from_ref(&only));

        assert_eq!(comparison.new.len(), 1);
        assert!(comparison.existing.is_empty());
        assert!(comparison.resolved.is_empty());
    }

    #[test]
    fn an_empty_current_run_resolves_everything() {
        let generator = IdGenerator::new(0);
        let only = finding(&generator, "src/a.rs", "hardcoded key");
        let comparison = compare(std::slice::from_ref(&only), &[]);

        assert_eq!(comparison.resolved.len(), 1);
        assert!(comparison.new.is_empty());
        assert!(comparison.existing.is_empty());
    }
}
