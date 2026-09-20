//! One identifier per unit of work, so two things happening at once read apart in the log.

use rand::Rng;

/// Sixteen hexadecimal characters: short enough to read in a log line, wide enough that
/// two concurrent units of work do not collide, and already the shape a W3C trace id
/// carries, so this survives a later move to OpenTelemetry.
pub fn new_id() -> String {
    format!("{:016x}", rand::rng().random::<u64>())
}

/// A trace id a caller supplied. It is echoed into every log line, so anything but a
/// plain identifier is refused rather than escaped: a newline in a log is a forged line.
pub fn accepted(value: &str) -> Option<&str> {
    let usable = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));

    usable.then_some(value)
}
