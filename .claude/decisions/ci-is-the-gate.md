# CI is the gate, and the local loop is its fast subset (7.0)

Until 7.0 no workflow ran clippy, fmt or nextest: the *Definition of done* had
three local tiers, and tier 3 — the whole workspace, the env-prefix run, the
feature matrix, `cargo audit` over three lockfiles, the docs lint, the demo's
e2e — ran "locally, by you" before a merge. A step a session had to remember
was a step it could skip, and nothing outside the machine noticed.

A survey of fifteen Rust framework and adapter repositories (2026-10-03) found
every one gating each pull request in CI with a fast local loop, expensive
suites ordered after cheap checks, and every backend adapter tested against
real backends (service containers, compose, testcontainers) — none mocks. So:

- **CI runs on every push and pull request and is the gate**, against real
  Postgres, Redis and Valkey versions, and S3.
- **The local loop is about a minute**: fmt through a Claude Code hook, the
  workspace clippy, nextest over `rdeps(<crate>)` without the trybuild
  snapshots and e2e, which join only when a macro crate or an adapter moved.
- **`cargo mutants` became advisory.** No surveyed peer gates on it; it stays a
  CI lane and an on-demand local tool, because a surviving mutant is still the
  best evidence of what a suite does not assert.
- **`cargo deny` replaced `cargo audit`** and `.cargo/audit.toml`, carrying the
  same ignores with their reasons, and adding the licence, ban and source checks
  six of the fifteen peers run.
- **The trybuild snapshots stay.** Peers that use trybuild pin the toolchain for
  them (axum, actix-web); ours is pinned by `rust-toolchain.toml`, so the
  snapshots change only on a deliberate bump.

**Nothing runs on a timer (2026-10-03).** The first cut ran the Redis/Valkey
matrix, the feature matrix, minimal versions and beta clippy nightly, and a
daily advisory watch. The owner refused machines running for days nobody
changed anything: a job now runs when a change reaches what it tests
(`backends.yml`, `features.yml` path filters); advisories between changes are
GitHub's Dependabot alerts, a repository setting; a dependency a coming Rust
release will reject is read from the clippy build CI already runs. Minimal
versions and beta clippy were dropped: the first cannot pass under the
`major.minor` rule, and nobody acts on an advisory lane.

**The local run is the gate, CI its safety net (2026-10-03).** Agents cannot
push, so a gate only CI could run was one no change reached before the owner's
branch; the gap was filled by a private script outside the repository. The
owner's line: an agent runs everything locally, and a check it cannot run
locally is a defect of the environment, since CI minutes are paid and a local
run is not. So the root `Justfile` runs every check CI runs (`just ci`, with
`just pre-commit` as its minute-long subset), the devcontainer pins the same
tools, and CI runs the same checks on push.
