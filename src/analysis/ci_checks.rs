use crate::confidence::Confidence;
use crate::signal::{NewSignal, SignalKey, SignalValue};

pub const ANALYZER: &str = "ci-check-runs";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Neutral,
    Skipped,
    Cancelled,
    TimedOut,
    ActionRequired,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub url: Option<String>,
    pub log_excerpt_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCheck {
    pub name: String,
    pub url: Option<String>,
    pub log_excerpt_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub signal: Option<NewSignal>,
    pub failed_checks: Vec<FailedCheck>,
}

pub fn classify(checks: &[CheckRun]) -> Classification {
    let failed_checks = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Completed && check.conclusion == Some(CheckConclusion::Failure))
        .map(|check| FailedCheck {
            name: check.name.clone(),
            url: check.url.clone(),
            log_excerpt_ref: check.log_excerpt_ref.clone(),
        })
        .collect::<Vec<_>>();
    let signal = (!failed_checks.is_empty()).then(|| NewSignal {
        key: SignalKey::TestsFailing,
        value: SignalValue::Count(failed_checks.len() as u64),
        confidence: Confidence::new(1.0).expect("confidence is in range"),
        reason: format!("{} completed CI checks failed.", failed_checks.len()),
    });
    Classification { signal, failed_checks }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, status: CheckStatus, conclusion: Option<CheckConclusion>) -> CheckRun {
        CheckRun {
            name: name.to_owned(),
            status,
            conclusion,
            url: Some(format!("https://ci.example.invalid/{name}")),
            log_excerpt_ref: Some(format!("log:{name}")),
        }
    }

    #[test]
    fn emits_signal_and_evidence_only_for_completed_failures() {
        let checks = [
            check("unit", CheckStatus::Completed, Some(CheckConclusion::Failure)),
            check("integration", CheckStatus::Completed, Some(CheckConclusion::Failure)),
            check("pending", CheckStatus::InProgress, Some(CheckConclusion::Failure)),
            check("build", CheckStatus::Completed, Some(CheckConclusion::Success)),
            check("lint", CheckStatus::Completed, Some(CheckConclusion::Cancelled)),
        ];
        let result = classify(&checks);
        assert_eq!(result.signal.as_ref().map(|signal| signal.value), Some(SignalValue::Count(2)));
        assert_eq!(result.failed_checks.len(), 2);
        assert_eq!(result.failed_checks[0].name, "unit");
        assert_eq!(result.failed_checks[0].url.as_deref(), Some("https://ci.example.invalid/unit"));
        assert_eq!(result.failed_checks[0].log_excerpt_ref.as_deref(), Some("log:unit"));
    }

    #[test]
    fn missing_conclusions_and_non_failures_are_clear() {
        let checks = [
            check("queued", CheckStatus::Queued, None),
            check("running", CheckStatus::InProgress, None),
            check("unknown", CheckStatus::Completed, None),
            check("neutral", CheckStatus::Completed, Some(CheckConclusion::Neutral)),
        ];
        let result = classify(&checks);
        assert!(result.signal.is_none());
        assert!(result.failed_checks.is_empty());
    }
}
