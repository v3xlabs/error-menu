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

The analyses API reads at most fifty snapshots per request. `before` uses the last snapshot ID
returned by `next_before` to read older entries. `kind` and `key` select one subject's history;
`latest=true` selects the newest reading per subject before pagination. Project dashboards use
that latest-per-subject view, and a subject page loads older readings on request. A scan response
loads only its requested snapshot.

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

The current worker runs in the app process and reads SQLite, forge credentials, and local mirrors.
Repository clone and fetch use gix transports, including an external SSH process. URL validation
does not constrain the address of the socket that gix opens: a domain can resolve to an internal
address after validation. Forge HTTP uses a resolver that rejects non-public addresses at
connection time, but that protection does not apply to gix or its SSH child process.

Before admitting untrusted project remotes, deploy the process and its children with an egress
policy that rejects loopback, private, link-local, and other non-public IPv4 and IPv6 destinations
at socket connect. Permit DNS through a controlled path. Verify the network implementation's
handling of pod-local loopback, redirects, and translated addresses; a Kubernetes NetworkPolicy
alone may not cover every one of these paths. A future separate worker process can have this
policy without restricting the app's local HTTP traffic.

## Work sources

Polling and webhooks produce the same job. Polling is the baseline. A webhook is a latency
optimisation on the same path, never a second code path: `POST /forge/github/events` calls
`Job::enqueue` exactly as the schedule does and wakes the queue, so a lost delivery costs
latency and the next poll finds the change anyway.

Webhooks come from the GitHub App, not from a hook added to each repository. GitHub signs
every delivery with the webhook secret in `X-Hub-Signature-256`, and that signature is the
endpoint's only authentication, so it sits outside the session guard. An install is learned
from the signed `installation` and `installation_repositories` deliveries, never from the
`installation_id` GitHub adds to the setup redirect, which anyone can forge.

## Reporting to the forge

A project reports to GitHub when three things hold: its owner turned `reports_to_forge` on,
the server has a GitHub App, and an installation of that app covers the repository. The
install gives permission and the switch decides, so an install on every repository of an
account turns nothing on by itself. Signing in to error.menu stays on the separate OAuth app
and asks for `read:user` alone: a reader never sees the install, only the owner who turns
reporting on does.

The server holds the app in `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY_FILE` and
`GITHUB_WEBHOOK_SECRET`, with `PUBLIC_ORIGIN` for the Details link. None set is a server that
never writes; some set is a startup error. Register the app by hand with the webhook URL
`PUBLIC_ORIGIN/forge/github/events`; repository permissions Checks read and write, Pull
requests read, Contents read and Metadata read; the events Pull request, Push, Check run and
Check suite; and user authorization during installation off. Only github.com repositories
can be covered.

A scan writes one check named `error.menu` on the head: queued when a webhook arrives,
in progress when the analysis starts, completed when it ends. The check carries counts and
the Details link, never a finding's title or location, because anyone can read a check on
a public repository. It is red for an introduced, untriaged High or Critical finding from any
analyzer, green when the change introduced nothing untriaged, and grey otherwise, including
a scan that could not finish. A write the forge refuses never fails the scan: the next pass
that finds the head without a completed check writes it again. Re-run on GitHub scans a pull
request head again; any other head gets a fresh check from the stored analysis.

The app writes and the worker never does: the app lives in `AppState` on the side that holds
the database. A repository an installation covers is also read with that installation's
token, asked for that one repository only, which gives it the installation's own request
budget. The mirror still clones anonymously.
