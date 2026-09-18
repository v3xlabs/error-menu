use std::path::Path;

/// Reads a file from `tests/samples`, resolved against the crate root rather than the
/// working directory, so it works from unit tests, integration tests and a test binary
/// run directly.
pub fn sample(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples")
        .join(relative);

    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read sample {}: {error}", path.display()))
}
