# Analyzers

An analyzer reads one snapshot and emits findings or signals. An analyzer does not write to a
forge or database. It does not decide how the API or web client presents its output.

## Execution boundary

No analyzer executes repository content. Repository files are untrusted input to bounded mirror
reads and parsers. error.menu does not run build scripts, package-manager hooks, compilers, tests,
or workflow commands.

## Implemented analyzers

`lockfile-delta` compares supported lockfiles. It reports package additions, removals, version
moves, changed sources, and changed integrity data. A flake input is named by its repository
(`github:NixOS/nixpkgs`, with `?host=` for a forge other than the public one), so an input that
moves to a new revision in a different repository is a changed source, not a version move.

`manifest-delta` compares `Cargo.toml` and `package.json` dependency declarations.

`secret-scan` examines added lines only. Each finding quotes the matched value and the pattern
it matched, because a match is already in the repository's history. The API links each finding
with a line to that line at the snapshot head on the project's forge.

It recognises the prefixed token formats of AWS, GitHub, GitLab, Slack, Stripe (live keys),
Google, OpenAI, Anthropic, npm, PyPI, Hugging Face, Shopify and SendGrid, and private key
blocks. A prefix counts only at the start of a word, and the body must have the length and
characters the issuer publishes. A line with none of these falls back to a high-entropy quoted
value assigned to a key whose name contains a word such as `secret`, `token` or `password`. Each
line reports at most one finding.

`link-inventory` inventories the full base and head trees. It preserves each link exactly as it
appears with its path, line, and site kind. It reports only added links and emits `LinksAdded`.

`workflow-security` reads changed GitHub workflow files. It reports concrete unpinned actions,
new write permissions, untrusted-trigger checkouts of pull-request code, and downloaded shell
pipelines.

`repository-controls` reports changes to `CODEOWNERS`, `.gitattributes`, `.gitmodules`, supported
package-manager controls, and dependency automation controls. Its `.gitmodules` report retains
the before and after submodule paths and URLs.

`ci-check-runs` reads CI facts from the configured forge at the snapshot head. It stores each
normalized check run and emits `TestsFailing` for completed failed checks. A project without a
supported forge skips this analyzer. A forge read failure marks only this analyzer failed. The
check error.menu writes itself is not CI and is left out, so its own verdict never comes back
as a failing test.

`repository-hygiene` reports changed generated-output directories and files over the size limit.

## Project selection

A project either uses the backend-owned default set or a custom set. The default set contains all
eight implemented analyzers. A custom project runs only its saved analyzer identifiers. Projects
that existed before default selection was added remain custom projects, so their analyzer sets do
not change during migration.

## Registry facts

No analyzer reads a package registry. A scan must not wait on somebody else's server, and
what a registry says about `serde 1.0.200` is the same answer for every project that locks
it, so `package_facts` is keyed by ecosystem, name and version alone and each version is
fetched by its own `package-facts` job, queued when an analysis records it. Only a package the lockfile
resolved from the public registry is fetched or shown with facts: a workspace member, a git
pin or a private registry package can share a public name and version and be different
bytes. Cargo facts come from the crates.io version and crate endpoints, paced to one
crates.io request per second, the docs.rs status file, and advisories from OSV, which carries
both the GitHub advisory database and RustSec; a GitHub record and its RustSec twin name each
other as aliases and are kept as one advisory. npm facts come from `registry.npmjs.org`, with
weekly downloads from `api.npmjs.org`, and install size, dependency count and advisories from
npmx.dev. npmx walks the whole install tree, and only the advisories published against the
package itself are kept, because each dependency is its own coordinate with its own facts.
The advisory read is required: a row without it would tell the audit nothing is known against
the version. Size and downloads are allowed to fail without failing the row. A flake input has
no registry, so it is never fetched.

A version the cache already answers is not read again, whichever project asked. A failed
read is cached for 15 minutes and the job retries after that time. A registry that answers
429 makes every job that reads it wait, and nothing else. A sweep every ten minutes queues
any version that has no answer, which is how the cache refills after it is emptied.

When a fetch completes the last answer a snapshot was waiting on, it queues a
`package-audit` job for that snapshot's project.

Package links (registry, docs, source) are sent as written, each with a status. A link is
`safe` only when sanitising it (https only, no credentials, no control characters, dot
segments or unencoded characters) leaves it unchanged. Any other link is `suspicious`: the
web client shows it in red with the raw URL in its tooltip, and does not make it followable.

`checksum-audit` and `package-audit` are run identifiers rather than selectable analyzers.
Both need the facts, which arrive after the analysis has finished, so the facts job records
each as its own run on the snapshot it audited: a finding still belongs to exactly one run
and no finished run is written to twice. A snapshot is audited only when every
public-registry package it names has a known or absent answer, because the run marks the
snapshot as done. Neither is in the default set and `evaluate` has no arm for them.

`checksum-audit` compares what a lockfile claims a package hashes to against what the
publisher published. `package-audit` looks at each public-registry version the snapshot
brings in (added, upgraded, downgraded or changed) and reports each advisory at its rating
(critical and high as themselves, moderate and unrated as medium, low and RustSec notices
such as `unmaintained` as low), a yanked or deprecated version as medium, and a version of
5 MB or more as medium. The size is the package's own published size (npm unpacked size, the
compressed Cargo crate), not its install tree: each dependency arrives as its own coordinate
and is weighed there. A removed version leaves with its problems and is not reported.

## Deferred work

LLM review is not implemented. Link policy, link normalization, and link allow lists are also
deferred. The link analyzer records facts only.
