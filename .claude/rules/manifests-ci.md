---
paths:
  - "Cargo.toml"
  - "crates/*/Cargo.toml"
  - "demo/**/Cargo.toml"
  - "bench/**/Cargo.toml"
  - "rust-toolchain.toml"
  - "clippy.toml"
  - ".cargo/**"
  - ".github/**"
  - "Justfile"
  - "CHANGELOG.md"
  - "changelog/**"
---

# Manifests, lints, CI and release

## Lints are workspace policy

- **`[workspace.lints]` is the one table, and every crate opts in** with
  `[lints] workspace = true`; no crate restates or narrows it. No lint there is
  `forbid`, so the site that must break one says so with
  `#[expect(lint, reason = "…")]`. `unreachable_pub` is the mechanical half of
  `architecture.md`'s *`pub` means exported*.
- **`clippy.toml` exists once, at the repository root.** clippy reads the
  nearest file upwards, so it covers `crates/`, `demo/` and `bench/`, and a
  closer one would *replace* it. A `disallowed-*` entry carries a `reason`
  naming what to use instead.
- **Tests are exempt by configuration**: the `allow-*-in-tests` keys, plus one
  attribute per suite root for the helpers those keys do not reach.

## Dependencies

- **Third-party versions live in `[workspace.dependencies]` only**; members
  write `dep = { workspace = true }`.
- **Freshness**: a new crate has a release in about twelve months. One that
  crosses the bar is flagged at its pin with the condition to move. No fork, no
  vendoring, no `[patch]` of a third-party crate: an upstream defect is
  reported upstream.
- **Every third-party requirement is `major.minor`** (`"1.53"`, `"=2.0"`), in
  every manifest the repo owns or the CLI generates: the minor is the floor we
  build against, the patch is the publisher's. The majors that surface in the
  macros' output move only with a nestrs major. Exact pins (`=`) say why at the
  pin; never bump one casually.
- **The framework requires itself in lockstep**: every `nest-rs-*` entry of
  the root `[workspace.dependencies]` is `=` the release, and a framework crate
  links a sibling only through `{ workspace = true }` — macro expansions and
  sibling crates call `__private`, which semver does not cover. Bump them with
  `[workspace.package] version`.
- **Intra-workspace dev-dependencies are path-only** (`{ path = "../nest-rs-x" }`),
  so publishing drags no test-only cycle; cargo strips them.
- **A manifest that consumes the framework names the umbrella and nothing
  else** — `nest-rs` with features. `nest-rs-macro-hygiene` compiles every
  decorator behind that one line.

## Shipping a capability

Sub-crates are compilation units, not the install surface. A capability ships
with all of this, or it is not shipped:

1. An umbrella feature pulling everything its decorators emit, and
   `pub use nest_rs_<x> as <x>;`.
2. `cargo add nest-rs --features <x>` in the crate README and the docs page's
   `## Install`.
3. Every derive its decorators emit routed through the surface crate with its
   `crate = ` override, so the use site declares neither the crate nor a
   version.
4. **The expansion witness**: a use site in `nest-rs-macro-hygiene`, gated on
   the capability's feature, and a snapshot per refusal in its suite.
   Decorators only.
5. **The composition witness**: a test in the capability's own crate booting
   the documented wiring through `nest_rs_testing::TestApp` (or
   `App::builder`), one per `for_root` seam.
6. **Every guard edge armed**: a crate implementing `Guard::check_<edge>`
   behind its own `<edge>` feature is enabled by the umbrella's `<edge>`
   feature as `<crate>?/<edge>`. Every `check_*` defaults to `Ok(())`, so a
   pairing left out is a guard that passes everything on that edge.

`demo/` is our `sample/`: a docs snippet with no counterpart in `demo/` or the
owning crate's suite is undocumented.

## Workspaces and toolchain

- `demo/` is its own workspace (`publish = false` crates) and never joins the
  root `members`; nothing at the root builds or tests it, and `demo.yml` runs
  its own recipes. `.cargo/config.toml`
  (mold) is inherited by `demo/`, never duplicated.
- `rust-toolchain.toml` pins the channel as `major.minor`; each workspace's
  `rust-version`, the images, `nestrs doctor`, the docs and every scaffold move
  with it in one change. The pin also keeps the trybuild snapshots still.
- `--workspace` builds one feature union, so a crate that compiles only
  because a sibling enabled a feature passes it. `just lint` builds the umbrella
  and `nest-rs-macro-hygiene` under each feature alone (`cargo hack`): the
  install contract is `cargo add nest-rs --features <x>`, not a sub-crate.

## CI

**CI runs on a change, never on a clock, and only what keeps the product from
regressing** (`decisions/ci-on-change.md`). `framework.yml` runs `just ci`,
recipe for recipe: fmt, clippy, each capability alone, the dependency policy,
rustdoc, then every test against real Postgres, Valkey and S3 — never a mock —
each on one server, the dev container's: a backend's other shapes run by hand
(`decisions/one-server-per-backend.md`).
`demo.yml` runs the demo's own recipes — `just lint` with the tree's `nestrs
lint`, and every suite — on a change to what its build or its services read:
its tree, the framework it builds on by path, the dev container files the
services start from. `docs.yml` builds the site on a change to `docs/` and
deploys it from `main` alone. The benchmarks are a developer's, on their host:
no workflow lints, audits or runs them. **A workflow skips only what no build
or test reads**: a file under a crate's `src/` is compiled whatever its
extension, and a docs page a test reads is listed in `framework.yml`'s paths.
The cargo cache is saved by `main` and `release/**` alone.

**A workflow is named for the tree a change to it triggers** (`Framework`,
`Demo`, `Docs`), **otherwise for what it does** (`Audit`, `Publish`); a job for
what it runs, with parentheses only for a closed set (`test (unit,
integration, doctests, e2e)`) — never a tool or a service, which grow. Held by
review.

`deny.toml` is the dependency policy: no known vulnerable, unsound or
unmaintained crate, no licence outside its list, nothing outside crates.io, no
crate it bans; an exception names its advisory or crate and its reason. **A
check belongs where only a change can turn it red**: the bans, licences and
sources are `just lint`'s, while the advisories, which the database moves
overnight, are `just audit`'s, over the framework's and the demo's lockfiles —
run by `audit.yml` on a change to a tree (blocking) and by `publish.yml` before
a release. The npm trees are third-party tooling taking their vendor's fixes as
they ship: `docs/` kept to what it uses, a bench SUT whole as its framework's
CLI scaffolds it (`decisions/npm-lockfile-advisories.md`). The workflows stay
hardened — actions pinned by SHA, a local action referenced `$/`
(`decisions/self-repository-actions.md`), `persist-credentials: false`, least
permissions — held by zizmor's offline audits and actionlint in `just lint`
(`decisions/workflow-lint.md`); an exception is a `# zizmor: ignore[…]` beside
its reason.

## Release

The tag equals the workspace `version`; `publish.yml` publishes. The changelog
follows Keep a Changelog, one file per minor line — `changelog/<major>.<minor>.md`
holds that line's releases and patches — and `CHANGELOG.md` is their index, newest
first. A new line opens its file under `[Unreleased]` and its index row; a breaking
change carries its upgrading entry. A released entry is the record of what shipped
and is never rewritten to a later rule; `[Unreleased]` is prose like any other,
`<PREFIX>_` included.
