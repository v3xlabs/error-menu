use crate::prelude::*;
use crate::vcs::mirror::{ChangedFile, FileChange};

pub const ANALYZER: &str = "repository-hygiene";

const LARGE_FILE_BYTES: u64 = 1_000_000;
const GENERATED_DIRECTORIES: [&str; 4] = ["node_modules", "dist", "out", "target"];

pub fn signals(
    changed_files: &[ChangedFile],
    size_at_head: impl Fn(&RepoPath) -> Option<u64>,
) -> Vec<NewSignal> {
    let generated_count = changed_files
        .iter()
        .filter(|changed| changed.change != FileChange::Deleted)
        .filter(|changed| is_generated_path(&changed.path))
        .count() as u64;
    let large_count = changed_files
        .iter()
        .filter(|changed| changed.change != FileChange::Deleted)
        .filter_map(|changed| size_at_head(&changed.path))
        .filter(|size| *size > LARGE_FILE_BYTES)
        .count() as u64;

    let mut signals = Vec::new();
    if generated_count > 0 {
        signals.push(NewSignal {
            key: SignalKey::RepositoryHygiene,
            value: SignalValue::Count(generated_count),
            confidence: Confidence::new(1.0).expect("confidence is in range"),
            reason: format!(
                "{generated_count} changed files are under generated-output directories."
            ),
        });
    }
    if large_count > 0 {
        signals.push(NewSignal {
            key: SignalKey::RepositoryHygiene,
            value: SignalValue::Count(large_count),
            confidence: Confidence::new(1.0).expect("confidence is in range"),
            reason: format!("{large_count} changed files exceed {LARGE_FILE_BYTES} bytes."),
        });
    }

    signals
}

fn is_generated_path(path: &RepoPath) -> bool {
    path.as_str()
        .split('/')
        .any(|segment| GENERATED_DIRECTORIES.contains(&segment))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            path: RepoPath::new(path).expect("path is valid"),
            change: FileChange::Added,
        }
    }

    #[test]
    fn flags_generated_output() {
        let changed_files = [changed("web/node_modules/example/index.js")];

        let signals = signals(&changed_files, |_| Some(100));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].value, SignalValue::Count(1));
    }

    #[test]
    fn flags_large_changed_files() {
        let changed_files = [changed("assets/archive.bin")];

        let signals = signals(&changed_files, |_| Some(LARGE_FILE_BYTES + 1));

        assert_eq!(signals.len(), 1);
        assert!(signals[0].reason.contains("exceed"));
    }

    #[test]
    fn ignores_deleted_generated_files() {
        let changed_files = [ChangedFile {
            path: RepoPath::new("dist/app.js").expect("path is valid"),
            change: FileChange::Deleted,
        }];

        assert!(signals(&changed_files, |_| Some(LARGE_FILE_BYTES + 1)).is_empty());
    }
}
