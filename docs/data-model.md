# Data model

An organization owns projects, and a project is the subject of everything below it. A finding is a
sighting, an issue is the persistent identity, and a signal is a snapshot-level opinion.

## Organization

An organization owns projects. `projects.organization_id` is `NOT NULL`, so every project has
exactly one, and `organization_members` holds one grant per `(organization_id, user_id)` pair with
the same three tiers a project grant uses.

```rust
struct Organization {
    id: Id<Organization>,
    name: String,
    description: Option<String>,
}
```

A caller's role on a project is the higher of the grant they hold on the project and the grant they
inherit from its organization. `ProjectRole` derives `Ord` for that comparison, so the declaration
order of `Viewer`, `Operator` and `Owner` is the privilege order and reordering it inverts the rule.
`ProjectRole::for_user` is the only place that resolves it, and every HTTP handler and MCP tool
reaches it through `project_access` or `readable_project`.

An organization keeps at least one owner. A project does not need its own owner row, because its
organization always has one.

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

