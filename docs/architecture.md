# Architecture

## Shape

The app owns SQLite, the schedule, the job queue, and the HTTP API. Workers lease a job over
HTTP, run analyzers, and post typed results back. The app and the worker are the same binary with
a role flag. Day one both run in one process; nothing about the design changes when they split.

## Storage

SQLite, with migrations from the first commit. The workload is rare and slow: a heavy user
watches a thousand repositories at roughly twenty change events a day each, which is a quarter
of a job per second, while one job takes seconds to minutes. SQLite handles this by three orders
of magnitude.

## Where a query lives

There is no data access layer. A query is a method on the thing it reads or writes, in
that thing's own module: `Job::claim`, `Snapshot::observe`, `ProjectMember::remove`,
`ApiToken::revoke`. `crate::database` owns only the pool, the id generator and
`DatabaseError`, so nothing accumulates a second copy of the domain's vocabulary, and a
reader looking for what can happen to a job opens the job.

Row spelling lives with the type too: every enum the database keeps as text implements
`StoredAs` and `FromStored` beside its definition, which keeps the stored spelling from
drifting into the API spelling.

Handlers and the worker are given one `AppState`: the database, the forge client, and the
two directories error.menu writes into. Passing state rather than a connection is what
lets a worker tick and a request run the same analysis code.

## Queue

The queue is a table in the same database, not a message broker.

```
jobs(id, project_id, subject_id, kind, priority, state,
     claimed_by, lease_expires_at, attempts, last_error,
     available_at, created_at)
```

A timer reclaims expired leases. Every job ends by writing analyzer results to the database, so
keeping the queue in that same database makes "store the results and mark the job done" one
transaction that cannot half happen. A broker would give at least once delivery, which forces
every analyzer to be idempotent anyway, and adds a failure mode where the job is acknowledged but
its results are lost. A table is also inspectable, which matters at three in the morning.

## What a poll costs

A poll finds the same commits nearly every time. Analysis is keyed by project and head in
`commit_analyses`, so a head that has been analysed is never analysed again: the sighting is
recorded, its snapshot points at the analysed one through `analysis_snapshot_id`, and the
reader sees the stored runs. Only a head nobody has analysed receives a pack. Forge metadata,
review state and CI status still refresh on every poll, because those change without a commit.

A subject holds one reading per commit it has pointed at. Discovery records a head the moment
it sees it, and a change that waits for its first scan is seen again on every poll, so the
analysis takes over the reading that has no run behind it rather than writing a second one
beside it. A reader asking for a re-scan of a head that already has runs gets a new reading:
that one is a fresh answer, not a repeat of the same sighting.

Anything git publishes is read from git. The default branch and its head come from the ref
advertisement that opens every connection to the remote, which costs one round trip, no
objects, and nothing from the forge's request budget. The forge is asked for one thing: the
changes open on top of the repository, with the titles, people and state that exist nowhere
in it. A poll therefore spends one forge request on the change list, plus one for each commit
whose signature is read and one for each head whose CI status is read.

## Forge credentials and budgets

`FORGE_TOKENS` holds `host=token` pairs separated by commas. The host is the one the API
answers on, not the one the repository is cloned from: a public GitHub repository is read
through `api.github.com`, while GitHub Enterprise, GitLab, Gitea and Forgejo answer under the
repository host itself. A token is sent to its own host and to no other, because the remote of
a project is chosen by whoever created it. A host without an entry is read anonymously, which
on `api.github.com` is sixty requests an hour for the whole source address. An exhausted
budget is not a failure: the forge says when it resets, the job waits until then, and the
attempt is given back. A refusal without a budget header is a failure, and it retries and
then stops.

## Authentication reads

Every authentication query rejects expired rows by predicate. Deleting those rows is
housekeeping and runs hourly on the schedule, never in a request, because SQLite has one
writer and a deletion that removes nothing still takes it.

## Where authorization resolves

One function answers what a caller may do with a project: `ProjectRole::for_user`. It takes the
higher of the direct `project_members` grant and the grant inherited from the project's
organization, and an administrator short circuits to owner before either is read. Every HTTP
handler reaches it through `project_access`, and every MCP tool through `readable_project`, so a
new endpoint gets the rule by calling one of those rather than by repeating it.

`Project::summaries_for` states the same rule a second time, in SQL, because a listing cannot ask
per row. That duplication is the sharp edge of this design: a change to who may see a project has
to land in both places or the project list and the project page will disagree.

Custody is a separate question from access. Changing which organization holds a project, or
deleting the project, resolves through `custody_access`, which requires an owner grant on
the organization that holds it. A project grant is handed out one repository at a time, and
anyone who may create an organization could otherwise move a project into one of their own.
A transfer is also the only write here that changes what `ProjectRole::for_user` answers for
a caller whose own grants never changed, which is why `Project::transfer` records the move
in the same transaction.

## Why no broker, ever

Kafka, RabbitMQ, Redis and BullMQ all solve throughput. Throughput is not our problem. If the
app ever needs to run on several machines, the split is not a broker: workers already hold no
database credentials and talk HTTP, so the app stays the single writer. Postgres only becomes
interesting if several apps must be live at once.

## Worker trust

Workers touch untrusted repository content. A worker can lease a job and post a result, and
nothing else. It has no database access, no forge credentials, and no host filesystem access.
This is a security boundary first and a scaling unit second.

## Work sources

Polling and webhooks produce the same job. Polling is the baseline. A webhook is a latency
optimisation on the same path, never a second code path. A webhook also requires changing
settings on the watched repository, which leaves a trace, so it is opt in per project.
