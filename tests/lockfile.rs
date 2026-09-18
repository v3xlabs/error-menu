mod common;

use common::sample;
use error_menu::analysis::lockfile::{Kind, delta};
use error_menu::finding::NewFinding;
use error_menu::vcs::RepoPath;

fn report(findings: &[NewFinding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| {
            format!(
                "{:?}: {} — {}",
                finding.severity, finding.title, finding.detail
            )
        })
        .collect()
}

fn run(kind: Kind, path: &str, base: &str, head: &str) -> Vec<NewFinding> {
    delta(
        kind,
        &RepoPath::new(path).expect("a valid path"),
        Some(&sample(base)),
        &sample(head),
    )
    .expect("parses")
}

#[test]
fn a_realistic_cargo_lockfile_parses_into_every_kind_of_entry() {
    let packages = Kind::Cargo
        .parse(&sample("lockfile/cargo/mixed-sources.base.lock"))
        .expect("parses");

    assert_eq!(packages.len(), 10);

    let members: Vec<_> = packages
        .iter()
        .filter(|package| package.source.is_none())
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(members, ["error-menu", "error-menu-worker"]);

    let unregistered: Vec<_> = packages
        .iter()
        .filter(|package| !package.registered)
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(unregistered, ["shared-helper"]);

    let syn: Vec<_> = packages
        .iter()
        .filter(|package| package.name == "syn")
        .map(|package| package.version.as_str())
        .collect();
    assert_eq!(syn, ["2.0.119", "3.0.5"]);
}

#[test]
fn a_realistic_cargo_change_reports_only_what_matters() {
    let findings = run(
        Kind::Cargo,
        "Cargo.lock",
        "lockfile/cargo/mixed-sources.base.lock",
        "lockfile/cargo/mixed-sources.head.lock",
    );

    insta::assert_debug_snapshot!(report(&findings));
}

#[test]
fn a_realistic_npm_lockfile_reads_scoped_and_nested_entries() {
    let packages = Kind::Npm
        .parse(&sample(
            "lockfile/npm/scoped-and-nested.base.package-lock.json",
        ))
        .expect("parses");

    let names: Vec<_> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();

    assert!(names.contains(&"@kobalte/core"));
    assert!(names.contains(&"@floating-ui/dom"));
    assert!(!names.contains(&"@error-menu/inner"));
    assert!(!names.contains(&""));
    assert_eq!(
        packages
            .iter()
            .filter(|package| package.name == "esbuild")
            .count(),
        2
    );
}

#[test]
fn a_realistic_npm_change_reports_only_what_matters() {
    let findings = run(
        Kind::Npm,
        "web/package-lock.json",
        "lockfile/npm/scoped-and-nested.base.package-lock.json",
        "lockfile/npm/scoped-and-nested.head.package-lock.json",
    );

    insta::assert_debug_snapshot!(report(&findings));
}

#[test]
fn a_realistic_flake_lock_skips_the_root_and_reads_its_inputs() {
    let packages = Kind::Nix
        .parse(&sample(
            "lockfile/nix/follows-and-git-input.base.flake.lock",
        ))
        .expect("parses");

    let names: Vec<_> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(names, ["flake-utils", "nixpkgs", "rust-overlay", "systems"]);
    assert!(packages.iter().all(|package| package.registered));
}

#[test]
fn a_realistic_flake_change_reports_only_what_matters() {
    let findings = run(
        Kind::Nix,
        "flake.lock",
        "lockfile/nix/follows-and-git-input.base.flake.lock",
        "lockfile/nix/follows-and-git-input.head.flake.lock",
    );

    insta::assert_debug_snapshot!(report(&findings));
}

#[test]
fn a_realistic_pnpm_lockfile_reads_its_explicit_document() {
    let packages = Kind::Pnpm
        .parse(&sample("lockfile/pnpm/framed-document.base.pnpm-lock.yaml"))
        .expect("parses");

    let names: Vec<_> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();

    assert_eq!(
        names,
        ["@solidjs/router", "internal-tool", "seroval", "solid-js"]
    );
    assert_eq!(
        packages
            .iter()
            .filter(|package| !package.registered)
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        ["internal-tool"]
    );
}

#[test]
fn a_realistic_pnpm_change_reports_only_what_matters() {
    let findings = run(
        Kind::Pnpm,
        "typescript/pnpm-lock.yaml",
        "lockfile/pnpm/framed-document.base.pnpm-lock.yaml",
        "lockfile/pnpm/framed-document.head.pnpm-lock.yaml",
    );

    insta::assert_debug_snapshot!(report(&findings));
}
