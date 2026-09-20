use crate::project::ProjectIcon;
use crate::vcs::mirror::{Mirror, MirrorError};
use crate::vcs::{CommitSha, RepoPath};

/// An icon is small. Anything larger is a screenshot or a banner that happens to sit at a
/// likely path, and serving it as a mark would be wrong anyway.
pub const MAX_ICON_BYTES: u64 = 512 * 1024;

/// A repository can hold tens of thousands of files. The walk stops well before that,
/// because a project does not keep its logo behind ten thousand other files.
const MAX_TREE_PATHS: usize = 20_000;
const MAX_DIMENSION_CANDIDATES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
    Either,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: RepoPath,
    pub scheme: Scheme,
    pub score: i32,
    shape_score: u8,
}

/// Directories a project puts things it means to publish. A file under one of these is
/// more likely to be the project's mark than one that happens to sit beside the source.
const PUBLISHED: &[&str] = &[
    "public", "assets", "static", "docs", "media", "images", "img", "brand", ".github", "web",
    "site", "www",
];

/// Directories holding someone else's files, or files that only exist for a test. A logo
/// is never in one of these, and a screenshot often is.
const IGNORED: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "fixtures",
    "__snapshots__",
    "coverage",
    ".git",
];

pub async fn infer(
    mirror: &Mirror,
    head: &CommitSha,
    project_name: &str,
) -> Result<ProjectIcon, MirrorError> {
    let candidates = suggest(mirror, head, project_name).await?;

    Ok(ProjectIcon {
        light: pick(&candidates, Scheme::Light),
        dark: pick(&candidates, Scheme::Dark),
    })
}

/// Every image in the repository that could be its mark, best first. The reader picks from
/// this when the ranking guesses wrong, which is why the whole list is kept rather than
/// only the winner.
pub async fn suggest(
    mirror: &Mirror,
    head: &CommitSha,
    project_name: &str,
) -> Result<Vec<Candidate>, MirrorError> {
    let mut candidates: Vec<Candidate> = mirror
        .paths_at(head, MAX_TREE_PATHS)
        .await?
        .into_iter()
        .filter_map(|path| {
            score(&path, project_name).map(|(scheme, score)| Candidate {
                path,
                scheme,
                score,
                shape_score: 0,
            })
        })
        .collect();

    sort_candidates(&mut candidates);
    for candidate in candidates.iter_mut().take(MAX_DIMENSION_CANDIDATES) {
        match mirror.bytes_at(head, &candidate.path, MAX_ICON_BYTES).await {
            Ok(Some(bytes)) => candidate.shape_score = shape_score(&bytes),
            Ok(None) | Err(MirrorError::TooLarge { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    sort_candidates(&mut candidates);

    Ok(candidates)
}

fn sort_candidates(candidates: &mut [Candidate]) {
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.shape_score.cmp(&left.shape_score))
            .then_with(|| left.path.as_str().len().cmp(&right.path.as_str().len()))
            .then_with(|| left.path.as_str().cmp(right.path.as_str()))
    });
}

fn shape_score(bytes: &[u8]) -> u8 {
    let Some((width, height)) = dimensions(bytes) else {
        return 0;
    };
    let largest = u64::from(width.max(height));
    let smallest = u64::from(width.min(height));

    u8::try_from(smallest * 100 / largest).unwrap_or(0)
}

fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png_dimensions(bytes);
    }
    if bytes.starts_with(b"\0\0\x01\0") {
        return ico_dimensions(bytes);
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return webp_dimensions(bytes);
    }

    svg_dimensions(bytes)
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);

    nonzero_dimensions(width, height)
}

fn ico_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let width = match *bytes.get(6)? {
        0 => 256,
        width => u32::from(width),
    };
    let height = match *bytes.get(7)? {
        0 => 256,
        height => u32::from(height),
    };

    nonzero_dimensions(width, height)
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match bytes.get(12..16)? {
        b"VP8X" => {
            let width =
                u32::from_le_bytes([*bytes.get(24)?, *bytes.get(25)?, *bytes.get(26)?, 0]) + 1;
            let height =
                u32::from_le_bytes([*bytes.get(27)?, *bytes.get(28)?, *bytes.get(29)?, 0]) + 1;

            nonzero_dimensions(width, height)
        }
        b"VP8 " if bytes.get(23..26) == Some(b"\x9d\x01\x2a") => {
            let width = u32::from(u16::from_le_bytes([*bytes.get(26)?, *bytes.get(27)?]) & 0x3fff);
            let height = u32::from(u16::from_le_bytes([*bytes.get(28)?, *bytes.get(29)?]) & 0x3fff);

            nonzero_dimensions(width, height)
        }
        b"VP8L" if bytes.get(20) == Some(&0x2f) => {
            let first = *bytes.get(21)?;
            let second = *bytes.get(22)?;
            let third = *bytes.get(23)?;
            let fourth = *bytes.get(24)?;
            let width = 1 + u32::from(first) + (u32::from(second & 0x3f) << 8);
            let height = 1
                + u32::from(second >> 6)
                + (u32::from(third) << 2)
                + (u32::from(fourth & 0x0f) << 10);

            nonzero_dimensions(width, height)
        }
        _ => None,
    }
}

/// Whether an SVG out of somebody else's repository can be handed to a browser. The bytes
/// are served from error.menu's own origin, so a document that can run script or reach
/// outside itself is refused rather than cleaned: sanitising markup invites a bypass, and a
/// mark never needs any of this.
pub fn svg_is_safe(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    if lower.contains("<script")
        || lower.contains("<foreignobject")
        || lower.contains("javascript:")
    {
        return false;
    }

    !has_event_attribute(&lower) && !has_external_reference(&lower)
}

/// An `on…=` attribute, whatever the event is called. Whitespace is allowed around the
/// equals sign, so `on load = "…"` is caught with `onload="…"`.
fn has_event_attribute(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut index = 0;
    while let Some(found) = lower[index..].find("on") {
        let at = index + found;
        index = at + 2;
        if at != 0 && !bytes[at - 1].is_ascii_whitespace() {
            continue;
        }
        let mut cursor = index;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            cursor += 1;
        }
        if cursor == index {
            continue;
        }
        if value_start(bytes, cursor).is_some() {
            return true;
        }
    }

    false
}

/// A reference that leaves the document, through `href`, `xlink:href`, `<image>` or
/// `<use>`. Two references stay: a fragment pointing back into the same file, and a
/// raster pasted into the attribute itself. Anything else, a value this does not
/// understand included, counts as external.
fn has_external_reference(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut index = 0;
    while let Some(found) = lower[index..].find("href") {
        index = index + found + 4;
        let Some(mut cursor) = value_start(bytes, index) else {
            continue;
        };
        let Some(quote @ (b'"' | b'\'')) = bytes.get(cursor).copied() else {
            return true;
        };
        cursor += 1;
        let Some(end) = bytes[cursor..].iter().position(|byte| *byte == quote) else {
            return true;
        };
        if !reaches_nothing(lower[cursor..cursor + end].trim()) {
            return true;
        }
        index = cursor + end;
    }

    false
}

/// Which drawing tools ship their own picture. A design tool writes a logo's raster parts
/// as base64 in the file, which fetches nothing and cannot be made to run anything, so
/// refusing those would refuse a large share of real marks.
///
/// Base64 is required, because a percent-encoded data URL carries arbitrary bytes, and
/// `image/svg+xml` is left out, because that nests a document this predicate has not read.
fn reaches_nothing(value: &str) -> bool {
    const EMBEDDED_RASTERS: [&str; 5] = ["png", "jpeg", "jpg", "gif", "webp"];

    if value.starts_with('#') {
        return true;
    }
    let Some((kind, raster)) = value
        .strip_prefix("data:image/")
        .and_then(|reference| reference.split_once(";base64,"))
    else {
        return false;
    };

    EMBEDDED_RASTERS.contains(&kind) && raster.bytes().all(is_base64)
}

fn is_base64(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte.is_ascii_whitespace() || matches!(byte, b'+' | b'/' | b'=')
}

/// Where an attribute's value starts, when the name ending at `cursor` is assigned one at
/// all. Whitespace is allowed on both sides of the equals sign.
fn value_start(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'=') {
        return None;
    }
    cursor += 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }

    Some(cursor)
}

fn svg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let start = text.find("<svg")?;
    let tag = &text[start..=start + text[start..].find('>')?];

    if let Some(view_box) = attribute(tag, "viewBox") {
        let mut values = view_box
            .split(|character: char| character.is_ascii_whitespace() || character == ',')
            .filter(|value| !value.is_empty());
        values.next()?;
        values.next()?;

        return nonzero_dimensions(
            positive_dimension(values.next()?)?,
            positive_dimension(values.next()?)?,
        );
    }

    nonzero_dimensions(
        positive_dimension(attribute(tag, "width")?)?,
        positive_dimension(attribute(tag, "height")?)?,
    )
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}="))? + name.len() + 1;
    let quote = tag.get(start..)?.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let value = tag.get(start + quote.len_utf8()..)?;
    let end = value.find(quote)?;

    value.get(..end)
}

fn positive_dimension(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.ends_with('%') {
        return None;
    }
    let end = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());

    value
        .get(..end)?
        .parse()
        .ok()
        .filter(|dimension| *dimension > 0)
}

fn nonzero_dimensions(width: u32, height: u32) -> Option<(u32, u32)> {
    (width > 0 && height > 0).then_some((width, height))
}

/// The best candidate for one scheme. A project with a single logo answers both schemes
/// with it, which is what most repositories have.
fn pick(candidates: &[Candidate], wanted: Scheme) -> Option<RepoPath> {
    candidates
        .iter()
        .find(|candidate| candidate.scheme == wanted)
        .or_else(|| {
            candidates
                .iter()
                .find(|candidate| candidate.scheme == Scheme::Either)
        })
        .map(|candidate| candidate.path.clone())
}

/// `None` for a path that is not an image at all. Otherwise a scheme and a score, where a
/// higher score means the file is more likely to be what the project puts on its own page.
fn score(path: &RepoPath, project_name: &str) -> Option<(Scheme, i32)> {
    let text = path.as_str().to_lowercase();
    let (stem, extension) = text.rsplit_once('.')?;
    let mut score = match extension {
        "svg" => 40,
        "png" => 30,
        "webp" => 20,
        "ico" => 10,
        _ => return None,
    };

    let segments: Vec<&str> = text.split('/').collect();
    if segments
        .iter()
        .any(|segment| IGNORED.contains(segment) || segment.starts_with("test"))
    {
        return None;
    }

    let name = segments.last()?;
    let directories = &segments[..segments.len() - 1];

    // A mark says so in its name. Without that the file is a screenshot, a diagram, or a
    // photograph that happens to sit in a likely directory, and a score alone cannot tell
    // those apart from a logo.
    let slug = project_name.to_lowercase();
    let named_for_project = !slug.is_empty() && name.contains(&slug);
    let purpose = if name.contains("logo") || name.contains("logotype") {
        60
    } else if name.contains("icon") || name.contains("mark") || name.contains("brand") {
        45
    } else if name.contains("favicon") {
        35
    } else if named_for_project {
        30
    } else {
        return None;
    };

    score += purpose;
    if named_for_project && purpose != 30 {
        score += 40;
    }

    if directories
        .iter()
        .any(|segment| PUBLISHED.contains(segment))
    {
        score += 20;
    }

    // A mark near the root is more likely the project's than one buried in an example.
    score -= i32::try_from(directories.len()).unwrap_or(0) * 5;

    if name.contains("screenshot") || name.contains("banner") || name.contains("diagram") {
        return None;
    }

    Some((scheme_of(stem), score))
}

fn scheme_of(stem: &str) -> Scheme {
    if stem.ends_with("dark") || stem.contains("dark_") || stem.contains("-dark-") {
        return Scheme::Dark;
    }

    if stem.ends_with("light") || stem.contains("light_") || stem.contains("-light-") {
        return Scheme::Light;
    }

    Scheme::Either
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(path: &str, project: &str) -> Option<(Scheme, i32)> {
        score(&RepoPath::new(path).expect("a usable path"), project)
    }

    #[test]
    fn a_logo_named_after_the_project_is_found_wherever_it_lives() {
        let (scheme, _) = scored("typescript/public/eth-prices_logo_light.svg", "eth-prices")
            .expect("a project logo is a candidate");

        assert_eq!(scheme, Scheme::Light);
    }

    #[test]
    fn the_dark_variant_is_told_apart_from_the_light_one() {
        assert_eq!(
            scored("docs/public/openlv_logo_dark.svg", "openlv").map(|found| found.0),
            Some(Scheme::Dark)
        );
        assert_eq!(
            scored("docs/public/openlv_logo_light.svg", "openlv").map(|found| found.0),
            Some(Scheme::Light)
        );
    }

    #[test]
    fn a_logo_outranks_a_favicon() {
        let logo = scored("assets/logo.svg", "acme").expect("a logo");
        let favicon = scored("assets/favicon.svg", "acme").expect("a favicon");

        assert!(logo.1 > favicon.1);
    }

    #[test]
    fn a_published_directory_outranks_a_buried_one() {
        let published = scored("public/logo.svg", "acme").expect("a published logo");
        let buried = scored("crates/inner/src/logo.svg", "acme").expect("a buried logo");

        assert!(published.1 > buried.1);
    }

    #[test]
    fn a_screenshot_is_not_a_mark() {
        assert_eq!(scored("docs/screenshot.png", "acme"), None);
    }

    #[test]
    fn someone_elses_files_are_never_a_mark() {
        assert_eq!(scored("node_modules/left-pad/logo.svg", "acme"), None);
        assert_eq!(scored("tests/fixtures/logo.svg", "acme"), None);
    }

    #[test]
    fn an_ordinary_picture_is_not_a_mark() {
        assert_eq!(scored("docs/architecture.png", "acme"), None);
        assert_eq!(scored("src/main.rs", "acme"), None);
    }

    #[test]
    fn one_logo_answers_both_schemes() {
        let candidates = vec![Candidate {
            path: RepoPath::new("assets/logo.svg").expect("a usable path"),
            scheme: Scheme::Either,
            score: 100,
            shape_score: 100,
        }];

        assert!(pick(&candidates, Scheme::Light).is_some());
        assert!(pick(&candidates, Scheme::Dark).is_some());
    }

    #[test]
    fn svg_view_box_sets_the_square_preference() {
        assert_eq!(shape_score(br#"<svg viewBox="0 0 100 100"></svg>"#), 100);
        assert_eq!(shape_score(br#"<svg viewBox="0, 0, 300, 100"></svg>"#), 33);
    }

    #[test]
    fn png_dimensions_set_the_square_preference() {
        let mut bytes = vec![0; 24];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[16..20].copy_from_slice(&256_u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&128_u32.to_be_bytes());

        assert_eq!(shape_score(&bytes), 50);
    }

    #[test]
    fn ico_dimensions_set_the_square_preference() {
        assert_eq!(shape_score(&[0, 0, 1, 0, 1, 0, 64, 64]), 100);
    }

    #[test]
    fn webp_dimensions_set_the_square_preference() {
        let mut bytes = vec![0; 30];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WEBP");
        bytes[12..16].copy_from_slice(b"VP8X");
        bytes[24..27].copy_from_slice(&[255, 1, 0]);
        bytes[27..30].copy_from_slice(&[255, 1, 0]);

        assert_eq!(shape_score(&bytes), 100);
    }

    #[test]
    fn square_shape_breaks_a_semantic_tie_without_overriding_it() {
        let mut candidates = vec![
            Candidate {
                path: RepoPath::new("assets/icon.svg").expect("a usable path"),
                scheme: Scheme::Either,
                score: 85,
                shape_score: 100,
            },
            Candidate {
                path: RepoPath::new("assets/logo_2x.png").expect("a usable path"),
                scheme: Scheme::Either,
                score: 100,
                shape_score: 25,
            },
            Candidate {
                path: RepoPath::new("assets/logo_yellow.svg").expect("a usable path"),
                scheme: Scheme::Either,
                score: 100,
                shape_score: 100,
            },
        ];

        sort_candidates(&mut candidates);

        let paths: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "assets/logo_yellow.svg",
                "assets/logo_2x.png",
                "assets/icon.svg"
            ]
        );
    }

    #[test]
    fn a_plain_mark_is_served() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke-linejoin="round"><path d="M4 4h16v16H4z"/><use href="#glyph"/></svg>"##;

        assert!(svg_is_safe(svg));
    }

    #[test]
    fn a_script_element_is_refused() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><SCRIPT>fetch("/api/users")</SCRIPT></svg>"#;

        assert!(!svg_is_safe(svg));
    }

    #[test]
    fn an_event_handler_is_refused_however_it_is_spaced() {
        let inline = br#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"></svg>"#;
        let spaced = br#"<svg xmlns="http://www.w3.org/2000/svg" OnLoad = 'alert(1)'></svg>"#;

        assert!(!svg_is_safe(inline));
        assert!(!svg_is_safe(spaced));
    }

    #[test]
    fn a_reference_outside_the_document_is_refused() {
        let remote = br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://example.invalid/pixel.png"/></svg>"#;
        let legacy = br#"<svg xmlns="http://www.w3.org/2000/svg"><use xlink:href="https://example.invalid/sprite.svg#glyph"/></svg>"#;

        assert!(!svg_is_safe(remote));
        assert!(!svg_is_safe(legacy));
    }

    #[test]
    fn a_raster_carried_in_the_document_is_served() {
        let png = br#"<svg xmlns="http://www.w3.org/2000/svg"><pattern id="p"><image xlink:href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUg=="/></pattern><rect fill="url(#p)" width="1" height="1"/></svg>"#;
        let wrapped = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"data:image/jpeg;base64,/9j/4AAQSkZJRg\n   AAAQ==\"/></svg>";

        assert!(svg_is_safe(png));
        assert!(svg_is_safe(wrapped));
    }

    /// A data URL is only inert while it is a raster and while its payload cannot be
    /// anything but base64: the rest carries markup, or a document of its own.
    #[test]
    fn a_data_url_that_is_not_a_raster_is_refused() {
        for svg in [
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="/></svg>"#.as_slice(),
            br#"<svg xmlns="http://www.w3.org/2000/svg"><a href="data:text/html;base64,PHNjcmlwdD48L3NjcmlwdD4="><path d="M0 0h1v1H0z"/></a></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/png,%3Csvg%20onload%3Dalert(1)%3E"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/png;base64,PHN2Zz4 onload=alert(1)"/></svg>"#,
        ] {
            assert!(!svg_is_safe(svg));
        }
    }

    #[test]
    fn a_javascript_url_is_refused() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><a href="javascript:alert(1)"><path d="M0 0h1v1H0z"/></a></svg>"#;

        assert!(!svg_is_safe(svg));
    }

    #[test]
    fn bytes_that_are_not_text_are_refused() {
        assert!(!svg_is_safe(&[0xff, 0xfe, 0x00]));
    }
}
