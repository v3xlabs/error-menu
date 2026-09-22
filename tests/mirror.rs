use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use error_menu::analysis::finding::{Ecosystem, Location, PackageOrigin, Severity};
use error_menu::analysis::lockfile;
use error_menu::vcs::mirror::{FileChange, Mirror, MirrorError};
use error_menu::vcs::{CommitSha, RemoteUrl, RepoPath};

/// A mirror holding two commits, the second swapping a checksum under an unchanged
/// version. Built through gitoxide straight into the directory the mirror would clone
/// into, so `Mirror::open` finds it and takes the already-cloned path.
///
/// Cloning and fetching are deliberately not exercised here. Both need a remote to talk
/// to, and gitoxide serves `file://` by spawning `git-upload-pack`, which is the one thing
/// this codebase is meant to avoid. They are covered against a real remote instead.
struct Fixture {
    root: PathBuf,
    remote: RemoteUrl,
    commits: Vec<CommitSha>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn build() -> Self {
        // A clock is not a unique name. These tests run concurrently and this clock
        // advances in ten-nanosecond steps, so two fixtures could land on one directory,
        // share a repository, and race each other's commits.
        static NEXT: AtomicU32 = AtomicU32::new(0);

        let root = PathBuf::from(".tmp/mirror-tests").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let remote = RemoteUrl::new("https://example.invalid/fixture").expect("a usable url");

        let path = Mirror::directory(&root.join("mirrors"), &remote);
        std::fs::create_dir_all(&path).expect("creates the mirror directory");

        let repository = gix::init_bare(&path).expect("initialises a bare repository");
        write_identity(&repository);
        let repository = gix::open(&path).expect("reopens with the identity in place");

        let mut commits = Vec::new();
        let mut parents: Vec<gix::ObjectId> = Vec::new();
        for checksum in ["aaa", "bbb"] {
            let blob = repository
                .write_blob(lockfile_text(checksum).as_bytes())
                .expect("writes the lockfile blob");

            let mut tree = gix::objs::Tree::empty();
            tree.entries.push(gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "Cargo.lock".into(),
                oid: blob.into(),
            });
            let tree = repository.write_object(&tree).expect("writes the tree");

            let commit = repository
                .commit("HEAD", checksum, tree, parents.clone())
                .expect("writes the commit")
                .detach();

            parents = vec![commit];
            commits.push(CommitSha::new(&commit.to_hex().to_string()).expect("a full sha"));
        }

        Self {
            root,
            remote,
            commits,
        }
    }

    fn with_tree(build: impl FnOnce(&gix::Repository) -> gix::ObjectId) -> Self {
        let mut fixture = Self::build();
        let path = Mirror::directory(&fixture.root.join("mirrors"), &fixture.remote);
        let repository = gix::open(path).expect("opens the fixture repository");
        let tree = build(&repository);
        let parent = gix::ObjectId::from_hex(fixture.commits[1].as_str().as_bytes())
            .expect("the fixture head is a full sha");
        let commit = repository
            .commit("HEAD", "tree traversal", tree, [parent])
            .expect("writes the tree commit")
            .detach();
        fixture.commits.push(
            CommitSha::new(&commit.to_hex().to_string()).expect("the tree commit is a full sha"),
        );
        fixture
    }

    async fn mirror(&self) -> Mirror {
        Mirror::open(&self.root.join("mirrors"), &self.remote)
            .await
            .expect("opens the mirror already on disk")
    }
}

/// Written into the repository's own config rather than the environment, so that whatever
/// the developer has configured cannot change what this fixture produces, and so that
/// tests running in parallel cannot see each other's identity.
fn write_identity(repository: &gix::Repository) {
    let path = repository.path().join("config");
    let mut config = std::fs::read_to_string(&path).unwrap_or_default();
    config.push_str("\n[user]\n\tname = Fixture\n\temail = fixture@example.invalid\n");
    std::fs::write(&path, config).expect("writes the repository config");
}

fn lockfile_text(checksum: &str) -> String {
    format!(
        "version = 4\n\n\
         [[package]]\n\
         name = \"serde\"\n\
         version = \"1.0.1\"\n\
         source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
         checksum = \"{checksum}\"\n"
    )
}

#[tokio::test]
async fn a_mirror_reads_two_revisions_and_the_gate_sees_the_swap() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let path = RepoPath::new("Cargo.lock").expect("a valid path");
    let base = mirror
        .file_at(&fixture.commits[0], &path)
        .await
        .expect("reads")
        .expect("the lockfile is in the first commit");
    let head = mirror
        .file_at(&fixture.commits[1], &path)
        .await
        .expect("reads")
        .expect("the lockfile is in the second commit");

    assert!(base.contains("\"aaa\""));
    assert!(head.contains("\"bbb\""));
}

#[tokio::test]
async fn the_gate_finds_the_swap_through_the_mirror_alone() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let findings = lockfile::delta_for_change(&mirror, &fixture.commits[0], &fixture.commits[1])
        .await
        .expect("analyses");

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].severity, Severity::Critical);
    assert_eq!(
        findings[0].location,
        Location::Package {
            path: RepoPath::new("Cargo.lock").expect("a valid path"),
            ecosystem: Ecosystem::Cargo,
            name: "serde".to_owned(),
            version: "1.0.1".to_owned(),
            origin: Some(PackageOrigin::PublicRegistry),
            integrity: Some("bbb".to_owned()),
        }
    );
}

#[tokio::test]
async fn a_mirror_reports_which_files_changed() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let changed = mirror
        .changed_files(&fixture.commits[0], &fixture.commits[1])
        .await
        .expect("diffs");

    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].path.as_str(), "Cargo.lock");
    assert_eq!(changed[0].change, FileChange::Modified);
}

#[tokio::test]
async fn a_mirror_answers_the_merge_base() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let merge_base = mirror
        .merge_base(&fixture.commits[0], &fixture.commits[1])
        .await
        .expect("answers");

    assert_eq!(merge_base, fixture.commits[0]);
}

#[tokio::test]
async fn a_missing_path_is_an_answer_and_not_a_failure() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let absent = mirror
        .file_at(
            &fixture.commits[0],
            &RepoPath::new("never/written.txt").expect("a valid path"),
        )
        .await
        .expect("reads");

    assert!(absent.is_none());
}

#[tokio::test]
async fn a_requested_path_limit_fails_instead_of_hiding_files() {
    let fixture = Fixture::with_tree(|repository| {
        let blob = repository
            .write_blob(b"contents")
            .expect("writes a blob")
            .detach();
        let mut tree = gix::objs::Tree::empty();
        for name in ["a.txt", "b.txt"] {
            tree.entries.push(gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: name.into(),
                oid: blob,
            });
        }
        repository
            .write_object(&tree)
            .expect("writes the tree")
            .detach()
    });
    let mirror = fixture.mirror().await;
    let revision = &fixture.commits[2];

    let paths = mirror
        .paths_at(revision, 2)
        .await
        .expect("lists both files");
    assert_eq!(
        paths,
        vec![
            RepoPath::new("a.txt").unwrap(),
            RepoPath::new("b.txt").unwrap()
        ]
    );
    assert!(matches!(
        mirror.paths_at(revision, 1).await,
        Err(MirrorError::TooManyPaths { limit: 1 })
    ));
}

#[tokio::test]
async fn repeated_empty_subtrees_count_toward_the_unlimited_path_walk() {
    let fixture = Fixture::with_tree(|repository| {
        let mut subtree = repository
            .write_object(gix::objs::Tree::empty())
            .expect("writes an empty tree")
            .detach();
        // Fifteen branching levels expand to 65,534 directory occurrences and no files.
        for _ in 0..15 {
            let mut tree = gix::objs::Tree::empty();
            for name in ["a", "b"] {
                tree.entries.push(gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Tree.into(),
                    filename: name.into(),
                    oid: subtree,
                });
            }
            subtree = repository
                .write_object(&tree)
                .expect("writes a shared subtree")
                .detach();
        }
        subtree
    });
    let mirror = fixture.mirror().await;

    assert!(matches!(
        mirror.paths_at(&fixture.commits[2], usize::MAX).await,
        Err(MirrorError::TooManyTreeEntries { limit: 20_000 })
    ));
}

#[tokio::test]
async fn tree_and_directory_entry_limits_include_the_boundary() {
    for count in [20_000, 20_001] {
        let fixture = Fixture::with_tree(|repository| {
            let blob = repository
                .write_blob(b"contents")
                .expect("writes a blob")
                .detach();
            let mut tree = gix::objs::Tree::empty();
            for index in 0..count {
                tree.entries.push(gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: format!("file-{index:05}").into(),
                    oid: blob,
                });
            }
            repository
                .write_object(&tree)
                .expect("writes the wide tree")
                .detach()
        });
        let mirror = fixture.mirror().await;
        let revision = &fixture.commits[2];
        let paths = mirror.paths_at(revision, usize::MAX).await;
        let entries = mirror.entries_at(revision, "").await;

        if count == 20_000 {
            let paths = paths.expect("accepts the exact traversal limit");
            let entries = entries.expect("accepts the exact directory limit");
            assert_eq!(paths.len(), count);
            assert_eq!(entries.len(), count);
            assert_eq!(paths[19_999].as_str(), "file-19999");
            assert_eq!(entries[19_999].name, "file-19999");
        } else {
            assert!(matches!(
                paths,
                Err(MirrorError::TooManyTreeEntries { limit: 20_000 })
            ));
            assert!(matches!(
                entries,
                Err(MirrorError::TooManyTreeEntries { limit: 20_000 })
            ));
        }
    }
}

#[tokio::test]
async fn repeated_long_directory_paths_have_a_byte_budget() {
    let fixture = Fixture::with_tree(|repository| {
        let mut subtree = repository
            .write_object(gix::objs::Tree::empty())
            .expect("writes an empty tree")
            .detach();
        for _ in 0..13 {
            let mut tree = gix::objs::Tree::empty();
            for name in ["a".repeat(64), "b".repeat(64)] {
                tree.entries.push(gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Tree.into(),
                    filename: name.into(),
                    oid: subtree,
                });
            }
            subtree = repository
                .write_object(&tree)
                .expect("writes a shared subtree")
                .detach();
        }
        subtree
    });
    let mirror = fixture.mirror().await;

    assert!(matches!(
        mirror.paths_at(&fixture.commits[2], usize::MAX).await,
        Err(MirrorError::TreePathsTooLarge)
    ));
}

#[tokio::test]
async fn a_log_reads_the_main_line_newest_first() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let line = mirror
        .log(&fixture.commits[1], 10)
        .await
        .expect("reads the log");

    assert_eq!(
        line.iter()
            .map(|commit| commit.summary.as_str())
            .collect::<Vec<_>>(),
        ["bbb", "aaa"]
    );
    assert_eq!(line[0].sha, fixture.commits[1]);
    assert_eq!(line[1].sha, fixture.commits[0]);
    assert_eq!(line[0].author.as_deref(), Some("Fixture"));
}

#[tokio::test]
async fn a_log_reads_no_more_commits_than_it_was_asked_for() {
    let fixture = Fixture::build();
    let mirror = fixture.mirror().await;

    let line = mirror
        .log(&fixture.commits[1], 1)
        .await
        .expect("reads the log");

    assert_eq!(line.len(), 1);
    assert_eq!(line[0].sha, fixture.commits[1]);
}
