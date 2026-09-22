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
supported forge skips this analyzer. A forge read failure marks only this analyzer failed.

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
job fills it after an analysis records package findings. Cargo facts come from the
crates.io version and crate endpoints and the docs.rs status file. npm facts come from
`registry.npmjs.org`, with install size and vulnerability counts from npmx.dev, which are
the only two answers nothing else gives cheaply; those two are allowed to fail without
failing the row, because npmx.dev does not document those routes. A flake input has no
registry, so it is never fetched.

`checksum-audit` is a run identifier rather than a selectable analyzer. It compares what a
lockfile claims a package hashes to against what the publisher published, which needs the
facts, which arrive after the analysis has finished. The facts job records it as its own
run on the snapshot it audited, so the finding still belongs to exactly one run and no
finished run is written to twice. It is absent from the default set and `evaluate` has no
arm for it.

## Deferred work

LLM review is not implemented. Link policy, link normalization, and link allow lists are also
deferred. The link analyzer records facts only.
