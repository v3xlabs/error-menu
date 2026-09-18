use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use error_menu::analysis::lockfile;
use error_menu::finding::{Ecosystem, Location, Severity};
use error_menu::vcs::mirror::{FileChange, Mirror};
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
