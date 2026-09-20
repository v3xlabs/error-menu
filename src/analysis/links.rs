use std::collections::BTreeMap;

use crate::analysis::finding::fingerprint::{Components, Fingerprint};
use crate::prelude::*;

pub const ANALYZER: &str = "link-inventory";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteKind {
    ContainerImage,
    Html,
    Lockfile,
    Manifest,
    Markdown,
    PlainText,
    WorkflowAction,
}

impl SiteKind {
    fn rule(self) -> &'static str {
        match self {
            Self::ContainerImage => "container-image",
            Self::Html => "html",
            Self::Lockfile => "lockfile",
            Self::Manifest => "manifest",
            Self::Markdown => "markdown",
            Self::PlainText => "plain-text",
            Self::WorkflowAction => "workflow-action",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ContainerImage => "container image",
            Self::Html => "HTML",
            Self::Lockfile => "lockfile",
            Self::Manifest => "manifest",
            Self::Markdown => "Markdown",
            Self::PlainText => "text",
            Self::WorkflowAction => "workflow action",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Link {
    pub text: String,
    pub path: RepoPath,
    pub line: u32,
    pub site: SiteKind,
}

/// One link that a file gained, however many times it appears there. A lockfile that names
/// one registry five hundred times is one fact about that file, not five hundred, and a
/// person triaging findings cannot read five hundred rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedLink {
    pub text: String,
    pub path: RepoPath,
    pub site: SiteKind,
    pub first_line: u32,
    pub occurrences: u32,
    /// How many times the file already carried it, so a link that merely repeats more often
    /// than before says so.
    pub previously: u32,
}

pub struct LinkReport {
    pub links: Vec<Link>,
    pub findings: Vec<NewFinding>,
    pub signals: Vec<NewSignal>,
}

pub fn inventory(path: &RepoPath, text: &str) -> Vec<Link> {
    let mut links = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let line_number = index as u32 + 1;
        let site = site_kind(path);

        if let Some(action) = workflow_action(line, path) {
            links.push(Link {
                text: action.to_owned(),
                path: path.clone(),
                line: line_number,
                site: SiteKind::WorkflowAction,
            });
        }

        if let Some(image) = container_image(line, path) {
            links.push(Link {
                text: image.to_owned(),
                path: path.clone(),
                line: line_number,
                site: SiteKind::ContainerImage,
            });
        }

        for link in urls(line) {
            links.push(Link {
                text: link.to_owned(),
                path: path.clone(),
                line: line_number,
                site,
            });
        }
    }

    links
}

/// Grouped by file, site and exact text: a link is new to a file when the file carries it
/// more often than it did, which also catches the second copy of a link that was already
/// there once.
pub fn added(base: &[Link], head: &[Link]) -> Vec<AddedLink> {
    let previous = counts(base);
    let mut added = Vec::new();

    for (key, (occurrences, first_line)) in counts(head) {
        let previously = previous.get(&key).map_or(0, |(count, _)| *count);

        if occurrences <= previously {
            continue;
        }

        let (path, site, text) = key;

        added.push(AddedLink {
            text,
            path,
            site,
            first_line,
            occurrences,
            previously,
        });
    }

    added
}

type LinkKey = (RepoPath, SiteKind, String);

fn counts(links: &[Link]) -> BTreeMap<LinkKey, (u32, u32)> {
    let mut counts: BTreeMap<LinkKey, (u32, u32)> = BTreeMap::new();

    for link in links {
        let key = (link.path.clone(), link.site, link.text.clone());

        counts
            .entry(key)
            .and_modify(|(count, first_line)| {
                *count += 1;
                *first_line = (*first_line).min(link.line);
            })
            .or_insert((1, link.line));
    }

    counts
}

pub fn report(base: &[Link], head: &[Link]) -> LinkReport {
    let added = added(base, head);
    let findings = added.iter().map(finding).collect::<Vec<_>>();
    let occurrences = added
        .iter()
        .map(|link| u64::from(link.occurrences - link.previously))
        .sum::<u64>();
    let signals = (occurrences > 0)
        .then(|| NewSignal {
            key: SignalKey::LinksAdded,
            value: SignalValue::Count(occurrences),
            confidence: Confidence::new(1.0).expect("one is in range"),
            reason: format!(
                "{occurrences} links were added across {} {}.",
                added.len(),
                if added.len() == 1 { "site" } else { "sites" }
            ),
        })
        .into_iter()
        .collect();

    LinkReport {
        links: head.to_vec(),
        findings,
        signals,
    }
}

fn site_kind(path: &RepoPath) -> SiteKind {
    let path = path.as_str();
    let name = path.rsplit('/').next().unwrap_or(path);

    if path.starts_with(".github/workflows/")
        && matches!(name, _ if name.ends_with(".yml") || name.ends_with(".yaml"))
    {
        return SiteKind::WorkflowAction;
    }
    if name.eq_ignore_ascii_case("Dockerfile") || name.ends_with(".Dockerfile") {
        return SiteKind::ContainerImage;
    }
    if name.ends_with(".md") || name.ends_with(".mdx") {
        return SiteKind::Markdown;
    }
    if name.ends_with(".html") || name.ends_with(".htm") {
        return SiteKind::Html;
    }
    if matches!(
        name,
        "Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "flake.lock"
    ) {
        return SiteKind::Lockfile;
    }
    if matches!(
        name,
        "Cargo.toml" | "package.json" | "pyproject.toml" | "package.yaml" | "flake.nix"
    ) {
        return SiteKind::Manifest;
    }

    SiteKind::PlainText
}

fn workflow_action<'a>(line: &'a str, path: &RepoPath) -> Option<&'a str> {
    if site_kind(path) != SiteKind::WorkflowAction {
        return None;
    }

    let (_, value) = line.split_once("uses:")?;
    let value = value.trim_start();
    let value = value.split('#').next()?.trim_end();
    let action = value.split_ascii_whitespace().next()?;

    (action.contains('/') && action.contains('@')).then_some(action)
}

fn container_image<'a>(line: &'a str, path: &RepoPath) -> Option<&'a str> {
    if site_kind(path) != SiteKind::ContainerImage {
        return None;
    }

    let trimmed = line.trim_start();
    let keyword = trimmed.get(..4)?;
    if !keyword.eq_ignore_ascii_case("from")
        || !trimmed
            .get(4..)
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(char::is_whitespace)
    {
        return None;
    }

    let mut values = trimmed[4..].split_ascii_whitespace();
    let first = values.next()?;
    let image = if first.starts_with("--") {
        values.next()?
    } else {
        first
    };

    (!image.starts_with("${") && image != "scratch").then_some(image)
}

fn urls(line: &str) -> impl Iterator<Item = &str> {
    let mut remaining = line;

    std::iter::from_fn(move || {
        let Some(start) = url_start(remaining) else {
            remaining = "";
            return None;
        };
        let candidate = &remaining[start..];
        let end = candidate
            .find(|character: char| {
                character.is_ascii_whitespace()
                    || matches!(character, '"' | '\'' | '<' | '>' | '(' | ')')
            })
            .unwrap_or(candidate.len());
        remaining = &candidate[end..];
        Some(&candidate[..end])
    })
}

fn url_start(value: &str) -> Option<usize> {
    value.char_indices().find_map(|(index, character)| {
        (character == 'h'
            && (value[index..].starts_with("http://") || value[index..].starts_with("https://")))
        .then_some(index)
    })
}

/// One finding per link and file. The occurrence is always zero because the group is the
/// identity, which also keeps the fingerprint stable when the same link moves down a file.
fn finding(link: &AddedLink) -> NewFinding {
    let location = Location::File {
        path: link.path.clone(),
        span: Some(LineSpan {
            start: link.first_line,
            end: link.first_line,
        }),
    };
    let title = format!("new {} link added: {}", link.site.label(), link.text);

    NewFinding {
        movement: None,
        fingerprint: Fingerprint::compute(&Components {
            analyzer: ANALYZER,
            rule: link.site.rule(),
            location: &location,
            title: &title,
            occurrence: 0,
        }),
        location,
        severity: Severity::Info,
        confidence: Confidence::new(1.0).expect("one is in range"),
        attribution: Attribution::Introduced,
        detail: detail(link),
        title,
    }
}

/// The file is the row a reader is already looking at, so the detail names the link and
/// where in the file it is, and leaves the path to the location.
fn detail(link: &AddedLink) -> String {
    match (link.occurrences, link.previously) {
        (1, 0) => format!("{} appears at line {}.", link.text, link.first_line),
        (occurrences, 0) => format!(
            "{} appears {occurrences} times, first at line {}.",
            link.text, link.first_line
        ),
        (occurrences, previously) => format!(
            "{} now appears {occurrences} times, up from {previously}, first at line {}.",
            link.text, link.first_line
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> RepoPath {
        RepoPath::new(value).expect("path is valid")
    }

    #[test]
    fn preserves_links_as_written_with_their_source_site() {
        let workflow = inventory(
            &path(".github/workflows/release.yml"),
            "uses: acme/release@v1 # pinned elsewhere\nurl: https://example.test/a?x=1\n",
        );

        assert_eq!(
            workflow,
            vec![
                Link {
                    text: "acme/release@v1".to_owned(),
                    path: path(".github/workflows/release.yml"),
                    line: 1,
                    site: SiteKind::WorkflowAction,
                },
                Link {
                    text: "https://example.test/a?x=1".to_owned(),
                    path: path(".github/workflows/release.yml"),
                    line: 2,
                    site: SiteKind::WorkflowAction,
                },
            ]
        );
    }

    #[test]
    fn preserves_container_references_and_html_urls() {
        let images = inventory(
            &path("Dockerfile"),
            "FROM --platform=linux/amd64 ghcr.io/acme/app:1\n",
        );
        let html = inventory(
            &path("public/index.html"),
            "<script src=\"https://cdn.test/app.js\"></script>\n",
        );

        assert_eq!(images[0].text, "ghcr.io/acme/app:1");
        assert_eq!(images[0].site, SiteKind::ContainerImage);
        assert_eq!(html[0].text, "https://cdn.test/app.js");
        assert_eq!(html[0].site, SiteKind::Html);
    }

    #[test]
    fn preserves_url_boundaries_and_embedded_schemes() {
        let text = "éhttp://one.test\t'https://two.test/a?x=1&y=2'\"http://three.test\"<https://four.test>(http://five.test) https://six.test,http://nested.test; https://seven.test\u{a0}suffix HTTPS://ignored.test https://";

        assert_eq!(
            urls(text).collect::<Vec<_>>(),
            vec![
                "http://one.test",
                "https://two.test/a?x=1&y=2",
                "http://three.test",
                "https://four.test",
                "http://five.test",
                "https://six.test,http://nested.test;",
                "https://seven.test\u{a0}suffix",
                "https://",
            ]
        );
    }

    #[test]
    fn preserves_every_url_when_only_one_scheme_repeats() {
        for url in ["http://example.test/path", "https://example.test/path"] {
            let text = format!("{url} ").repeat(20_000);
            assert_eq!(urls(&text).collect::<Vec<_>>(), vec![url; 20_000]);
        }
    }

    #[test]
    fn reports_only_added_links_and_counts_them() {
        let source = path("README.md");
        let base = inventory(&source, "[docs](https://example.test/docs)\n");
        let head = inventory(
            &source,
            "[docs](https://example.test/docs)\n[status](https://status.example.test/now)\n",
        );

        let report = report(&base, &head);

        assert_eq!(report.links, head);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(
            report.findings[0].title,
            "new Markdown link added: https://status.example.test/now"
        );
        assert_eq!(report.signals[0].key, SignalKey::LinksAdded);
        assert_eq!(report.signals[0].value, SignalValue::Count(1));
    }

    #[test]
    fn one_link_repeated_in_one_file_is_one_finding_with_a_count() {
        let source = path("Cargo.lock");
        let registry = "https://github.com/rust-lang/crates.io-index";
        let base = inventory(&source, "");
        let head = inventory(
            &source,
            &format!(
                "source = \"registry+{registry}\"\nname = \"a\"\nsource = \"registry+{registry}\"\n"
            ),
        );

        let report = report(&base, &head);

        assert_eq!(report.findings.len(), 1);
        assert_eq!(
            report.findings[0].detail,
            format!("{registry} appears 2 times, first at line 1.")
        );
        assert_eq!(report.signals[0].value, SignalValue::Count(2));
    }

    #[test]
    fn a_link_that_repeats_more_often_than_before_is_reported_once() {
        let source = path("README.md");
        let link = "https://example.test/docs";
        let base = inventory(&source, &format!("[docs]({link})\n"));
        let head = inventory(&source, &format!("[docs]({link})\n[again]({link})\n"));

        let added = added(&base, &head);

        assert_eq!(added.len(), 1);
        assert_eq!(added[0].occurrences, 2);
        assert_eq!(added[0].previously, 1);
        assert_eq!(
            report(&base, &head).findings[0].detail,
            format!("{link} now appears 2 times, up from 1, first at line 1.")
        );
    }

    #[test]
    fn a_link_that_only_moves_down_a_file_is_not_added() {
        let source = path("README.md");
        let link = "https://example.test/docs";
        let base = inventory(&source, &format!("[docs]({link})\n"));
        let head = inventory(&source, &format!("\n\n[docs]({link})\n"));

        assert!(added(&base, &head).is_empty());
    }

    #[test]
    fn does_not_normalize_distinct_link_text() {
        let source = path("README.md");
        let base = inventory(&source, "[docs](https://example.test/docs)\n");
        let head = inventory(&source, "[docs](https://example.test/docs/)\n");

        assert_eq!(added(&base, &head).len(), 1);
    }
}
