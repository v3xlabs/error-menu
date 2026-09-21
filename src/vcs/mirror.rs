use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gix::bstr::{BStr, ByteSlice};
use gix::diff::tree::{Recorder, Visit, visit};

use crate::prelude::*;
use crate::vcs::RepoPathError;

/// Lockfiles are far smaller than this. The cap exists because zlib reaches roughly
/// 1032:1, so a few megabytes of pack can decompress to gigabytes, and a hostile pull
/// request only has to put such a blob where a gate will read it.
const MAX_BLOB_BYTES: u64 = 8 * 1024 * 1024;

/// A tree can reference the same subtree many times, so a few kilobytes of objects can
/// enumerate an enormous number of paths. gitoxide leaves this bound to the caller.
const MAX_CHANGED_FILES: usize = 5_000;

const MAX_TREE_ENTRIES: usize = 20_000;
const MAX_TREE_PATH_BYTES: usize = 8 * 1024 * 1024;

const FETCH_SECONDS: u64 = 300;

/// A bare mirror of one remote, read through gitoxide. Nothing is ever checked out, and
/// blob conversion runs in `ToGit` mode, so `.gitattributes` may name a diff driver but
/// its command is never run.
pub struct Mirror {
    repository: gix::ThreadSafeRepository,
}

#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("{action} failed: {source}")]
    Git {
        action: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("the work to read the mirror did not finish: {0}")]
    Interrupted(#[from] tokio::task::JoinError),
    #[error("the remote advertises no usable default branch")]
    NoDefaultBranch,
    #[error("{path} is {size} bytes at that revision, over the {MAX_BLOB_BYTES} byte limit")]
    TooLarge { path: String, size: u64 },
    #[error("{path} is not text at that revision")]
    NotText { path: String },
    #[error("the change touches more than {MAX_CHANGED_FILES} files")]
    TooManyChanges,
    #[error("the tree contains more than {limit} entries, including directories")]
    TooManyTreeEntries { limit: usize },
    #[error("the tree contains more than {limit} file paths")]
    TooManyPaths { limit: usize },
    #[error("the tree paths exceed the {MAX_TREE_PATH_BYTES} byte limit")]
    TreePathsTooLarge,
    #[error("a path in the diff is not valid text")]
    PathNotText,
    #[error("the diff named an unusable path: {source}")]
    Path {
        #[source]
        source: RepoPathError,
    },
}

fn failed(
    action: &'static str,
) -> impl FnOnce(Box<dyn std::error::Error + Send + Sync>) -> MirrorError {
    move |source| MirrorError::Git { action, source }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: String,
    pub path: RepoPath,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: RepoPath,
    pub change: FileChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Modified,
    Deleted,
}

/// What git alone knows about a repository's main line: the branch the remote advertises
/// as `HEAD`, and the commit it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultBranch {
    pub name: String,
    pub head: CommitSha,
}

impl Mirror {
    /// Where the mirror of this remote lives. The directory is named after a hash of the
    /// url, so a url can never steer where the mirror is written.
    pub fn directory(root: &Path, remote: &RemoteUrl) -> PathBuf {
        root.join(
            blake3::hash(remote.as_str().as_bytes())
                .to_hex()
                .to_string(),
        )
    }

    pub async fn open(root: &Path, remote: &RemoteUrl) -> Result<Self, MirrorError> {
        let path = Self::directory(root, remote);
        let url = remote.as_str().to_owned();
        let root = root.to_owned();
        let deadline = deadline(FETCH_SECONDS);

        let repository = blocking(move || {
            if path.join("HEAD").exists() {
                return Ok(gix::open_opts(&path, open_options())
                    .map_err(|source| failed("opening the mirror")(Box::new(source)))?
                    .into_sync());
            }

            std::fs::create_dir_all(&root)?;
            let (repository, _) = gix::clone::PrepareFetch::new(
                url,
                &path,
                gix::create::Kind::Bare,
                gix::create::Options::default(),
                open_options(),
            )
            .map_err(|source| failed("preparing the clone")(Box::new(source)))?
            .fetch_only(gix::progress::Discard, &deadline)
            .map_err(|source| failed("cloning")(Box::new(source)))?;

            Ok(repository.into_sync())
        })
        .await?;

        Ok(Self { repository })
    }

    pub fn path(&self) -> &Path {
        self.repository.path()
    }

    pub async fn fetch(&self) -> Result<(), MirrorError> {
        let repository = self.repository.clone();
        let deadline = deadline(FETCH_SECONDS);

        blocking(move || {
            let repository = repository.to_thread_local();
            repository
                .find_remote("origin")
                .map_err(|source| failed("finding the origin remote")(Box::new(source)))?
                .connect(gix::remote::Direction::Fetch)
                .map_err(|source| failed("connecting to the remote")(Box::new(source)))?
                .prepare_fetch(
                    gix::progress::Discard,
                    gix::remote::ref_map::Options::default(),
                )
                .map_err(|source| failed("preparing the fetch")(Box::new(source)))?
                .receive(gix::progress::Discard, &deadline)
                .map_err(|source| failed("fetching")(Box::new(source)))?;
            Ok(())
        })
        .await
    }

    /// The branch the remote calls its own default, and the commit it points at, taken
    /// from the ref advertisement every connection begins with. The connection is dropped
    /// before a pack is negotiated, so this costs one round trip and no objects: a poll
    /// that finds a head it has already analysed never pays for the rest.
    pub async fn default_branch(&self) -> Result<DefaultBranch, MirrorError> {
        let repository = self.repository.clone();

        blocking(move || {
            // A connection asks only for the ref prefixes its refspecs need, and the
            // mirror's fetch every branch. `HEAD` is not under any of them, so without
            // naming it the one ref this reads is the one the remote leaves out.
            let head =
                gix::refspec::parse(b"HEAD".as_bstr(), gix::refspec::parse::Operation::Fetch)
                    .map_err(|source| failed("reading the HEAD refspec")(Box::new(source)))?
                    .into();
            let repository = repository.to_thread_local();
            let listed = repository
                .find_remote("origin")
                .map_err(|source| failed("finding the origin remote")(Box::new(source)))?
                .connect(gix::remote::Direction::Fetch)
                .map_err(|source| failed("connecting to the remote")(Box::new(source)))?
                .prepare_fetch(
                    gix::progress::Discard,
                    gix::remote::ref_map::Options {
                        extra_refspecs: vec![head],
                        ..Default::default()
                    },
                )
                .map_err(|source| failed("listing the remote refs")(Box::new(source)))?;

            let advertised =
                listed
                    .ref_map()
                    .remote_refs
                    .iter()
                    .find_map(|advertised| match advertised {
                        gix::protocol::handshake::Ref::Symbolic {
                            full_ref_name,
                            target,
                            object,
                            ..
                        } if full_ref_name.as_slice() == b"HEAD".as_slice() => {
                            Some((target, object))
                        }
                        _ => None,
                    });
            let Some((target, object)) = advertised else {
                return Err(MirrorError::NoDefaultBranch);
            };
            let Some(name) = target.strip_prefix(b"refs/heads/".as_slice()) else {
                return Err(MirrorError::NoDefaultBranch);
            };
            let name = name.to_str().map_err(|_| MirrorError::NoDefaultBranch)?;

            Ok(DefaultBranch {
                name: name.to_owned(),
                head: CommitSha::new(&object.to_hex().to_string()).map_err(|source| {
                    failed("reading the default branch head")(Box::new(source))
                })?,
            })
        })
        .await
    }

    pub async fn merge_base(
        &self,
        base: &CommitSha,
        head: &CommitSha,
    ) -> Result<CommitSha, MirrorError> {
        let repository = self.repository.clone();
        let (base, head) = (base.clone(), head.clone());

        blocking(move || {
            let repository = repository.to_thread_local();
            let found = repository
                .merge_base(object_id(&base)?, object_id(&head)?)
                .map_err(|source| failed("finding the merge base")(Box::new(source)))?;

            CommitSha::new(&found.detach().to_hex().to_string())
                .map_err(|source| failed("reading the merge base")(Box::new(source)))
        })
        .await
    }

    pub async fn fetch_ref(&self, ref_name: &str) -> Result<(), MirrorError> {
        let repository = self.repository.clone();
        let ref_name = ref_name.to_owned();
        let deadline = deadline(FETCH_SECONDS);

        blocking(move || {
            let destination = format!(
                "refs/error-menu/discovery/{}",
                blake3::hash(ref_name.as_bytes()).to_hex()
            );
            let spec = format!("+{ref_name}:{destination}");
            let refspec = gix::refspec::parse(
                spec.as_bytes().as_bstr(),
                gix::refspec::parse::Operation::Fetch,
            )
            .map_err(|source| failed("reading a pull request ref")(Box::new(source)))?
            .into();
            let repository = repository.to_thread_local();
            repository
                .find_remote("origin")
                .map_err(|source| failed("finding the origin remote")(Box::new(source)))?
                .connect(gix::remote::Direction::Fetch)
                .map_err(|source| failed("connecting to the remote")(Box::new(source)))?
                .prepare_fetch(
                    gix::progress::Discard,
                    gix::remote::ref_map::Options {
                        extra_refspecs: vec![refspec],
                        ..Default::default()
                    },
                )
                .map_err(|source| failed("preparing a pull request fetch")(Box::new(source)))?
                .receive(gix::progress::Discard, &deadline)
                .map_err(|source| failed("fetching a pull request")(Box::new(source)))?;
            Ok(())
        })
        .await
    }

    /// Whether the commit object itself carries a signature. error.menu never checks the
    /// cryptography, so this reports only that a signature is there to be checked.
    pub async fn commit_signature(&self, sha: &CommitSha) -> Result<bool, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let commit = repository
                .find_object(object_id(&sha)?)
                .map_err(|source| failed("finding a commit")(Box::new(source)))?
                .try_into_commit()
                .map_err(|source| failed("reading a commit")(Box::new(source)))?;
            let decoded = commit
                .decode()
                .map_err(|source| failed("reading a commit")(Box::new(source)))?;

            Ok(decoded.extra_headers().pgp_signature().is_some())
        })
        .await
    }
    /// The people a commit names: its author and committer, plus whoever the trailers
    /// credit. Every one of these is written by the client that made the commit, so this
    /// reports what the commit claims rather than who really wrote it.
    pub async fn commit_people(&self, sha: &CommitSha) -> Result<Vec<Person>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let commit = repository
                .find_object(object_id(&sha)?)
                .map_err(|source| failed("finding a commit")(Box::new(source)))?
                .try_into_commit()
                .map_err(|source| failed("reading a commit")(Box::new(source)))?;

            let mut people = Vec::new();
            if let Ok(author) = commit.author() {
                people.push(signature(PersonRole::Author, author.name, author.email));
            }
            if let Ok(committer) = commit.committer() {
                people.push(signature(
                    PersonRole::Committer,
                    committer.name,
                    committer.email,
                ));
            }

            let message = commit
                .message()
                .map_err(|source| failed("reading a commit message")(Box::new(source)))?;
            if let Some(body) = message.body() {
                for trailer in body.trailers() {
                    let role = match trailer.token.to_str_lossy().to_ascii_lowercase().as_str() {
                        "co-authored-by" => PersonRole::CoAuthor,
                        "signed-off-by" => PersonRole::SignedOffBy,
                        "reviewed-by" => PersonRole::Reviewer,
                        _ => continue,
                    };
                    people.push(trailer_person(role, trailer.value.as_ref()));
                }
            }

            Ok(crate::project::person::deduplicate(people))
        })
        .await
    }
    pub async fn first_parent(&self, sha: &CommitSha) -> Result<Option<CommitSha>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let commit = repository
                .find_object(object_id(&sha)?)
                .map_err(|source| failed("finding a commit")(Box::new(source)))?
                .try_into_commit()
                .map_err(|source| failed("reading a commit")(Box::new(source)))?;
            let Some(parent) = commit.parent_ids().next() else {
                return Ok(None);
            };

            CommitSha::new(&parent.to_hex().to_string())
                .map(Some)
                .map_err(|source| failed("reading a commit parent")(Box::new(source)))
        })
        .await
    }

    /// Renames are reported as a deletion and an addition, because the tree diff used here
    /// does not track them. The convenience diff does, but it builds a resource cache from
    /// HEAD before it compares anything: it walks the whole tree, reads `.gitattributes`
    /// out of it, and refuses any tree holding a path component like `git~1`, which is a
    /// legal name on Linux. All of that is driven by whoever opened the change, and none of
    /// it is needed to learn which paths moved.
    pub async fn changed_files(
        &self,
        base: &CommitSha,
        head: &CommitSha,
    ) -> Result<Vec<ChangedFile>, MirrorError> {
        let repository = self.repository.clone();
        let (base, head) = (base.clone(), head.clone());

        blocking(move || {
            let repository = repository.to_thread_local();
            let base_tree = tree_at(&repository, &base)?;
            let head_tree = tree_at(&repository, &head)?;

            let mut collector = Collector::default();
            let diffed = gix::diff::tree(
                gix::objs::TreeRefIter::from_bytes(&base_tree.data, base_tree.id.kind()),
                gix::objs::TreeRefIter::from_bytes(&head_tree.data, head_tree.id.kind()),
                &mut gix::diff::tree::State::default(),
                &repository.objects,
                &mut collector,
            );

            // A refusal stops the walk, so gitoxide reports it as a cancellation. Ours is
            // the accurate one and has to be read first.
            if let Some(error) = collector.refused {
                return Err(error);
            }
            diffed.map_err(|source| failed("diffing")(Box::new(source)))?;

            Ok(collector.changes)
        })
        .await
    }

    /// Reads a blob as bytes, for pictures rather than text. The caller gives the cap
    /// because an icon has no business being as large as a lockfile.
    pub async fn bytes_at(
        &self,
        sha: &CommitSha,
        path: &RepoPath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();
        let path = path.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let tree = tree_at(&repository, &sha)?;
            let Some(entry) = tree
                .lookup_entry_by_path(path.as_str())
                .map_err(|source| failed("looking up the path")(Box::new(source)))?
            else {
                return Ok(None);
            };
            if !entry.mode().is_blob() {
                return Ok(None);
            }

            let header = repository
                .find_header(entry.id().detach())
                .map_err(|source| failed("reading the object header")(Box::new(source)))?;
            if header.size() > limit {
                return Err(MirrorError::TooLarge {
                    path: path.as_str().to_owned(),
                    size: header.size(),
                });
            }

            let object = entry
                .object()
                .map_err(|source| failed("reading the blob")(Box::new(source)))?;

            Ok(Some(object.data.clone()))
        })
        .await
    }

    /// The immediate children of one directory, directories first. An empty path lists the
    /// root, which is where browsing starts.
    pub async fn entries_at(
        &self,
        sha: &CommitSha,
        directory: &str,
    ) -> Result<Vec<TreeEntry>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();
        let directory = directory.trim_matches('/').to_owned();

        blocking(move || {
            let repository = repository.to_thread_local();
            let root = tree_at(&repository, &sha)?;
            let tree = if directory.is_empty() {
                root
            } else {
                let Some(entry) = root
                    .lookup_entry_by_path(directory.as_str())
                    .map_err(|source| failed("looking up the path")(Box::new(source)))?
                else {
                    return Ok(Vec::new());
                };
                let Ok(object) = entry.object() else {
                    return Ok(Vec::new());
                };
                match object.try_into_tree() {
                    Ok(tree) => tree,
                    Err(_) => return Ok(Vec::new()),
                }
            };

            let mut entries = Vec::new();
            let mut budget = TreeBudget::default();
            for entry in tree.iter() {
                let entry = entry.map_err(|source| failed("reading the tree")(Box::new(source)))?;
                let name = entry.filename().to_str_lossy();
                budget
                    .observe(directory.len() + usize::from(!directory.is_empty()) + name.len())?;
                let name = name.into_owned();
                let full = if directory.is_empty() {
                    name.clone()
                } else {
                    format!("{directory}/{name}")
                };
                let Ok(path) = RepoPath::new(&full) else {
                    continue;
                };
                let mode = entry.mode();
                if !mode.is_tree() && !mode.is_blob() {
                    continue;
                }

                entries.push(TreeEntry {
                    name,
                    path,
                    is_directory: mode.is_tree(),
                });
            }

            entries.sort_by(|left, right| {
                right
                    .is_directory
                    .cmp(&left.is_directory)
                    .then_with(|| left.name.cmp(&right.name))
            });

            Ok(entries)
        })
        .await
    }
    /// Every file in the tree at that revision. Exceeding the file limit or the internal
    /// traversal limits fails the entire enumeration rather than returning partial paths.
    pub async fn paths_at(
        &self,
        sha: &CommitSha,
        limit: usize,
    ) -> Result<Vec<RepoPath>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let tree = tree_at(&repository, &sha)?;
            let mut collector = PathCollector {
                limit,
                ..Default::default()
            };
            let walked = tree.traverse().breadthfirst(&mut collector);
            if let Some(error) = collector.refused {
                return Err(error);
            }
            walked.map_err(|source| failed("walking the tree")(Box::new(source)))?;

            Ok(collector.paths)
        })
        .await
    }

    pub async fn file_size_at(
        &self,
        sha: &CommitSha,
        path: &RepoPath,
    ) -> Result<Option<u64>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();
        let path = path.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let tree = tree_at(&repository, &sha)?;
            let Some(entry) = tree
                .lookup_entry_by_path(path.as_str())
                .map_err(|source| failed("looking up the path")(Box::new(source)))?
            else {
                return Ok(None);
            };
            if !entry.mode().is_blob() {
                return Ok(None);
            }

            repository
                .find_header(entry.id().detach())
                .map(|header| Some(header.size()))
                .map_err(|source| failed("reading the object header")(Box::new(source)))
        })
        .await
    }

    /// `None` means the path is not a file in that revision, which is an answer rather
    /// than a failure. The size is read from the object header, so an oversized blob is
    /// refused before anything decompresses it.
    pub async fn file_at(
        &self,
        sha: &CommitSha,
        path: &RepoPath,
    ) -> Result<Option<String>, MirrorError> {
        let repository = self.repository.clone();
        let sha = sha.clone();
        let path = path.clone();

        blocking(move || {
            let repository = repository.to_thread_local();
            let tree = tree_at(&repository, &sha)?;

            let Some(entry) = tree
                .lookup_entry_by_path(path.as_str())
                .map_err(|source| failed("looking up the path")(Box::new(source)))?
            else {
                return Ok(None);
            };

            if !entry.mode().is_blob() {
                return Ok(None);
            }

            let header = repository
                .find_header(entry.id().detach())
                .map_err(|source| failed("reading the object header")(Box::new(source)))?;
            if header.size() > MAX_BLOB_BYTES {
                return Err(MirrorError::TooLarge {
                    path: path.as_str().to_owned(),
                    size: header.size(),
                });
            }

            let object = entry
                .object()
                .map_err(|source| failed("reading the blob")(Box::new(source)))?;

            String::from_utf8(object.detach().data)
                .map(Some)
                .map_err(|_| MirrorError::NotText {
                    path: path.as_str().to_owned(),
                })
        })
        .await
    }
}

#[derive(Default)]
struct TreeBudget {
    entries: usize,
    path_bytes: usize,
}

impl TreeBudget {
    fn observe(&mut self, path_bytes: usize) -> Result<(), MirrorError> {
        if self.entries >= MAX_TREE_ENTRIES {
            return Err(MirrorError::TooManyTreeEntries {
                limit: MAX_TREE_ENTRIES,
            });
        }
        if path_bytes > MAX_TREE_PATH_BYTES - self.path_bytes {
            return Err(MirrorError::TreePathsTooLarge);
        }
        self.entries += 1;
        self.path_bytes += path_bytes;
        Ok(())
    }
}

#[derive(Default)]
struct PathCollector {
    recorder: gix::traverse::tree::Recorder,
    budget: TreeBudget,
    paths: Vec<RepoPath>,
    limit: usize,
    refused: Option<MirrorError>,
}

impl PathCollector {
    fn observe(&mut self, entry: &gix::objs::tree::EntryRef<'_>) -> Result<(), MirrorError> {
        self.budget.observe(self.recorder.path().len())?;
        if entry.mode.is_blob()
            && let Ok(path) = RepoPath::new(&self.recorder.path().to_str_lossy())
        {
            if self.paths.len() >= self.limit {
                return Err(MirrorError::TooManyPaths { limit: self.limit });
            }
            self.paths.push(path);
        }
        Ok(())
    }
}

impl gix::traverse::tree::Visit for PathCollector {
    fn pop_back_tracked_path_and_set_current(&mut self) {
        self.recorder.pop_back_tracked_path_and_set_current();
    }

    fn pop_front_tracked_path_and_set_current(&mut self) {
        self.recorder.pop_front_tracked_path_and_set_current();
    }

    fn push_back_tracked_path_component(&mut self, component: &BStr) {
        self.recorder.push_back_tracked_path_component(component);
    }

    fn push_path_component(&mut self, component: &BStr) {
        self.recorder.push_path_component(component);
    }

    fn pop_path_component(&mut self) {
        self.recorder.pop_path_component();
    }

    fn visit_tree(
        &mut self,
        entry: &gix::objs::tree::EntryRef<'_>,
    ) -> gix::traverse::tree::visit::Action {
        self.visit_nontree(entry)
    }

    fn visit_nontree(
        &mut self,
        entry: &gix::objs::tree::EntryRef<'_>,
    ) -> gix::traverse::tree::visit::Action {
        match self.observe(entry) {
            Ok(()) => ControlFlow::Continue(true),
            Err(error) => {
                self.refused = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

/// The tree diff reports a path one component at a time, so the delegate keeps the path and
/// the running answer together.
#[derive(Default)]
struct Collector {
    recorder: Recorder,
    changes: Vec<ChangedFile>,
    refused: Option<MirrorError>,
}

impl Collector {
    fn refuse(&mut self, error: MirrorError) -> visit::Action {
        self.refused = Some(error);
        ControlFlow::Break(())
    }
}

impl Visit for Collector {
    fn pop_front_tracked_path_and_set_current(&mut self) {
        self.recorder.pop_front_tracked_path_and_set_current();
    }

    fn push_back_tracked_path_component(&mut self, component: &BStr) {
        self.recorder.push_back_tracked_path_component(component);
    }

    fn push_path_component(&mut self, component: &BStr) {
        self.recorder.push_path_component(component);
    }

    fn pop_path_component(&mut self) {
        self.recorder.pop_path_component();
    }

    fn visit(&mut self, change: visit::Change) -> visit::Action {
        if self.changes.len() >= MAX_CHANGED_FILES {
            return self.refuse(MirrorError::TooManyChanges);
        }

        // A lossy conversion would let a changed lockfile hide behind a path that no gate
        // then recognises.
        let Ok(location) = self.recorder.path().to_str().map(str::to_owned) else {
            return self.refuse(MirrorError::PathNotText);
        };

        let path = match repo_path(&location) {
            Ok(path) => path,
            Err(error) => return self.refuse(error),
        };

        self.changes.push(ChangedFile {
            path,
            change: match change {
                visit::Change::Addition { .. } => FileChange::Added,
                visit::Change::Deletion { .. } => FileChange::Deleted,
                visit::Change::Modification { .. } => FileChange::Modified,
            },
        });

        ControlFlow::Continue(())
    }
}

/// Bounds per-object decompression. gitoxide leaves this unset when it trusts the
/// directory, and it trusts ours because we own it. `isolated` keeps the machine's system
/// and global git config, its `include.path` chains, and every `GIT_*` variable out of a
/// repository whose contents we do not trust. There is no private remote to authenticate
/// to, so the credential and proxy config lost with it is not wanted either.
fn open_options() -> gix::open::Options {
    gix::open::Options::isolated().config_overrides(["gitoxide.objects.allocLimit=64m"])
}

/// gitoxide takes a flag rather than a deadline, and bounds neither bytes nor time on its
/// own, so without this a slow or hostile remote holds a worker for ever.
fn deadline(seconds: u64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let timer = Arc::clone(&flag);

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        timer.store(true, Ordering::Relaxed);
    });

    flag
}

async fn blocking<T, F>(work: F) -> Result<T, MirrorError>
where
    F: FnOnce() -> Result<T, MirrorError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await?
}

fn object_id(sha: &CommitSha) -> Result<gix::ObjectId, MirrorError> {
    gix::ObjectId::from_hex(sha.as_str().as_bytes())
        .map_err(|source| failed("reading a revision")(Box::new(source)))
}

fn tree_at<'repository>(
    repository: &'repository gix::Repository,
    sha: &CommitSha,
) -> Result<gix::Tree<'repository>, MirrorError> {
    repository
        .find_object(object_id(sha)?)
        .map_err(|source| failed("finding the commit")(Box::new(source)))?
        .try_into_commit()
        .map_err(|source| failed("reading the commit")(Box::new(source)))?
        .tree()
        .map_err(|source| failed("reading the tree")(Box::new(source)))
}

fn repo_path(raw: &str) -> Result<RepoPath, MirrorError> {
    RepoPath::new(raw).map_err(|source| MirrorError::Path { source })
}

fn signature(role: PersonRole, name: &BStr, email: &BStr) -> Person {
    Person {
        role,
        name: text(name),
        email: text(email),
        login: None,
        avatar_url: None,
    }
}

fn trailer_person(role: PersonRole, value: &BStr) -> Person {
    let (name, email) = crate::project::person::parse_identity(&value.to_str_lossy());

    Person {
        role,
        name,
        email,
        login: None,
        avatar_url: None,
    }
}

fn text(value: &BStr) -> Option<String> {
    let value = value.to_str_lossy().trim().to_owned();

    (!value.is_empty()).then_some(value)
}
