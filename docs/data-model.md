# Data model

A finding is a sighting, an issue is the persistent identity, and a signal is a snapshot-level
opinion.

## Finding

One analyzer run saw a finding at one location. The location is required because the fingerprint
needs a stable subject. A dependency finding uses a package coordinate so lockfile regeneration
does not create a new issue.

```rust
struct Finding {
    id: FindingId,
    run_id: RunId,
    issue_id: IssueId,
    fingerprint: Fingerprint,
    location: Location,
    severity: Severity,
    confidence: Confidence,
    attribution: Attribution,
    title: String,
    detail: String,
}
```

## Issue

An issue is resolved by `(project_id, fingerprint)`. It holds the human triage decision across
runs. Its `analyzer` records who produced the fingerprint.

```rust
struct Issue {
    id: IssueId,
    project_id: ProjectId,
    analyzer: String,
    fingerprint: Fingerprint,
    triage: Triage,
    first_seen: SnapshotId,
    last_seen: SnapshotId,
}
```

Issue lifecycle is derived by comparing successful runs for the same analyzer and subject. A
failed run is never a comparison baseline.

## Signal

A signal is an analyzer opinion about the change as a whole. Signals have no location or
fingerprint and are not deduplicated.

```rust
enum SignalKey {
    OffTask,
    DiffSize,
    BlastRadius,
    DependencyRisk,
    TestsFailing,
    LinksAdded,
    RepositoryHygiene,
}

enum SignalValue {
    Score(f32),
    Flag(bool),
    Count(u64),
}
```

## CI check run

The forge reader normalizes check runs for the snapshot head. Each row stores a name, status,
optional conclusion, URL, and optional log reference. `ci-check-runs` reads those snapshot facts
and emits `TestsFailing` only when a completed check concludes with failure. Check-run storage is
separate from the analyzer run so a reader can inspect the source facts and the conclusion.

## Deferred evidence

The data model reserves evidence for tool output, model rationales, and log excerpts. Evidence
storage is not implemented. Check-run URLs and log references are the only persisted evidence-like
references today.

