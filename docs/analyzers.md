# Analyzers

An analyzer reads one snapshot and emits findings or signals. An analyzer does not write to a
forge or database. It does not decide how the API or web client presents its output.

## Execution boundary

No analyzer executes repository content. Repository files are untrusted input to bounded mirror
reads and parsers. error.menu does not run build scripts, package-manager hooks, compilers, tests,
or workflow commands.

## Implemented analyzers

`lockfile-delta` compares supported lockfiles. It reports package additions, removals, version
moves, changed sources, and changed integrity data.

`manifest-delta` compares `Cargo.toml` and `package.json` dependency declarations.

`secret-scan` examines added lines only. It reports credential-shaped values without storing the
matched value.

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
it, so `package_facts` is keyed by ecosystem, name and version alone and a `package-facts`
job fills it after an analysis records package findings. Only a package the lockfile
resolved from the public registry is fetched or shown with facts: a workspace member, a git
pin or a private registry package can share a public name and version and be different
bytes. Cargo facts come from the crates.io version and crate endpoints, paced to one
crates.io request per second, and the docs.rs status file. npm facts come from
`registry.npmjs.org`, with weekly downloads from `api.npmjs.org` and install size and
vulnerability counts from npmx.dev; those three reads are allowed to fail without failing
the row. A flake input has no registry, so it is never fetched.

One job reads at most 200 coordinates and stops after half the job lease; what is left
queues another job. A failed read is cached for 15 minutes and fails the job attempt, so the
job retries after that time.

Package links (registry, docs, source) are sent as written, each with a status. A link is
`safe` only when sanitising it (https only, no credentials, no control characters, dot
segments or unencoded characters) leaves it unchanged. Any other link is `suspicious`: the
web client shows it in red with the raw URL in its tooltip, and does not make it followable.

`checksum-audit` is a run identifier rather than a selectable analyzer. It compares what a
lockfile claims a package hashes to against what the publisher published, which needs the
facts, which arrive after the analysis has finished. The facts job records it as its own
run on the snapshot it audited, so the finding still belongs to exactly one run and no
finished run is written to twice. A snapshot is audited only when every public-registry
package with a lockfile integrity has a known or absent answer, because the run marks the
snapshot as done. It is absent from the default set and `evaluate` has no arm for it.

## Deferred work

LLM review is not implemented. Link policy, link normalization, and link allow lists are also
deferred. The link analyzer records facts only.
