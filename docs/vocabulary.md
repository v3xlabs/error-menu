# Vocabulary

**Remote** — a git remote. Refs, objects, diffs, blobs. The same on every host. We keep a bare
mirror per project and read all content from it. Diffs never come from a forge API, because
forge diffs are rate limited and truncated for large changes.

**Forge** — everything built on top of git: pull requests, issues, comments, review state,
check runs and their logs. Forge access is read only. A project can have no forge at all.

**Project** — one watched repository. It holds the remote, optional forge link, enabled analyzers,
and the schedule.

**Subject** — what we analyse. A change (pull request), a branch, or one commit. A subject
lives a long time. Pull request 42 is one subject through all of its pushes.

**Snapshot** — an immutable reading of a subject at one moment: head sha, base sha, merge base,
and the forge metadata as it was then. A new head sha makes a new snapshot. Forge metadata is
frozen here because people edit pull request titles, and a score that reads the title must stay
reproducible.

**Run** — one analyzer executed against one snapshot. A run records its own status, so "the
analyzer failed" can never be read as "the analyzer found nothing".

**Analyzer** — one unit of analysis. It takes a snapshot and emits findings or signals.

**App** — the main binary. Owns SQLite, the schedule, the job queue, and the HTTP API that the
web UI reads.

**Worker** — a process that leases a job, runs analyzers, and posts results back over HTTP. Holds
no database credentials.
