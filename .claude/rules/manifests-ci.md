---
paths:
  - "Cargo.toml"
  - "crates/*/Cargo.toml"
  - "demo/**/Cargo.toml"
  - "bench/**/Cargo.toml"
  - "rust-toolchain.toml"
  - "clippy.toml"
  - "scripts/**"
  - ".cargo/**"
  - "deny.toml"
  - ".config/**"
  - ".github/**"
  - "CHANGELOG.md"
---

# Manifests, lints, CI and release

## Lints are workspace policy

- **`[workspace.lints]` is the one table, and every crate opts in** with
  `[lints] workspace = true`; no crate restates or narrows it. A lint there is
  `deny`, never `forbid`, so the site that must break one can say so with
  `#[expect(lint, reason = "…")]` (`CLAUDE.md`, *How a rule is held*).
  `unreachable_pub` is the mechanical half of `architecture.md`'s *`pub` means
  exported*: an item no `lib.rs` re-export reaches is `pub(crate)` or private.
- **`clippy.toml` exists once, at the repository root.** clippy reads the
  nearest file upwards from each crate, so the root file covers `crates/`,
  `demo/` and `bench/` — and a closer one would *replace* it rather than merge.
  Never add one below the root. A `disallowed-*` entry ships with its canary
  in `nest-rs-macro-hygiene` (an entry whose path stops resolving is only a
  warning) and a `reason` naming what to use instead.
- **Tests are exempt by configuration, not by attribute sprawl**: the
  `allow-*-in-tests` keys in `clippy.toml`, and one attribute per suite root
  for what those keys do not reach.

## Dependencies

- **Third-party versions live in `[workspace.dependencies]` only**; members
  write `dep = { workspace = true }`.
- **Freshness** (`CLAUDE.md`, hard "no"): a new crate has a release in about
  twelve months. An existing one that crosses the bar is **flagged at its pin**
  with the evidence and the condition to move, never kept silently —
  `apalis` / `apalis-redis` is the one kept past it, argued in the root
  manifest. No fork, no vendoring, no `[patch]` of a third-party crate: an
  upstream defect is reported upstream.
- **Exact pins (`=`) carry their bump procedure in the comment above them**;
  follow it, never bump casually.

### `major.minor` — the one requirement form

**Every third-party requirement has exactly two components** (`"1.53"`,
`"=2.0"`), in every manifest the repo owns or the CLI generates: the minor is
the floor we build against, the patch is the publisher's. The minor moves with
`cargo update`, in the same change as the lock. The pinned majors listed in the
root manifest are the public surface the macros emit, so their bump ships in a
nestrs major; any other semver-incompatible release is reported, never taken.
The one exception is argued at its pin (`async-graphql`). Held by
`versions_are_major_minor` in `nest-rs-cli`, whose suite it lives in because a
scaffold inherits these pins verbatim.

### The framework requires itself in lockstep

**Every `nest-rs-*` entry of the root `[workspace.dependencies]` is `=` the
release**, and a framework crate links a sibling only through
`{ workspace = true }`: a `*-macros` crate emits calls into `#[doc(hidden)]`
seams semver does not cover, so a partial `cargo update -p` could pair one
crate's expansion with another's seams. A consumer requires `nest-rs = "7.0"`
and the umbrella moves every crate at once. Bump them with
`[workspace.package] version`; held by
`the_framework_requires_itself_at_its_own_release` in `nest-rs-cli`.

**Intra-workspace dev-dependencies are path-only** —
`{ path = "../nest-rs-x" }`, no `version`, no `workspace = true` — so
publishing drags no test-only cycle and a dev-edge never falls back to an
index that lacks the crate. Cargo strips them from the published manifest.

### One `nest-rs*` line per consumer

**A manifest that consumes the framework names the umbrella and nothing
else** (`CLAUDE.md`, *The umbrella is the front door*). Held by
`consumers_name_only_the_umbrella` in `nest-rs-cli`, over every consumer the
repo owns — `demo/`, `nest-rs-macro-hygiene` and `bench/sut/nestrs`, whose own
empty `[workspace]` no `--workspace` command reaches; a consumer created outside
both workspaces joins that list the day it exists.

### Shipping a capability

Sub-crates are compilation units, not the install surface, and renaming the
`nest-rs` dependency is unsupported, as with tokio. A capability ships with all
of this, or it is not shipped:

1. An umbrella feature pulling everything its decorators emit, and
   `pub use nest_rs_<x> as <x>;`.
2. `cargo add nest-rs --features <x>` in the crate README and the docs page's
   `## Install` (held by the docs lint).
3. Every derive its decorators emit routed through the surface crate with its
   `crate = ` override, so the use site declares neither the crate nor a
   version.
4. **The expansion witness**: a use site in `nest-rs-macro-hygiene`, whose one
   dependency is `nest-rs`, gated on the capability's feature so a misgated
   re-export fails under that feature alone. Decorators only.
5. **The composition witness**: a test in the capability's own crate booting
   the documented wiring through `nest_rs_testing::TestApp` (or
   `App::builder`) and asserting what a caller gets back, one per `for_root`
   seam. A boot through the real transport is the evidence that a route is
   mounted.

`demo/` is our `sample/`: a docs snippet with no counterpart in `demo/` or the
owning crate's suite is undocumented.

### Workspaces

- Product crates under `demo/` set `publish = false`; `demo/` is its own
  workspace and never joins the root `members`.
- `.cargo/config.toml` (the mold linker) is inherited by `demo/`
  hierarchically — never duplicated.

## The Rust floor is one value

`rust-toolchain.toml` pins the channel as a bare `major.minor`, and every other
site restating it — each workspace's `rust-version`, the images, the
workflows, `nestrs doctor`, the docs, every scaffold — moves with it in one
change. Held by `toolchain_pins_agree` and
`every_workspace_root_declares_the_floor` in `nest-rs-cli`. A benchmark's
recorded `rustc` is a measurement, not a pin, and never moves with the floor.
The pin is also what keeps the trybuild snapshots still (`testing.md`).

## Feature isolation and supply chain

**Every local build is one feature union.** `--workspace` unifies every
member's features and the hygiene witness enables all of them, so a crate that
compiles only because a sibling turned a feature on passes both.
`scripts/check-features.sh` is the build that sees it — every crate alone under
each of its features, then the umbrella with each feature alone — and CI runs
it.

**The supply chain is `cargo deny`**, configured by `deny.toml` at the root
(`.claude/decisions/ci-is-the-gate.md`). Every ignore there is argued, one
reason per entry, and revisited on every dependency bump.

## CI

**CI is the gate.** It runs on every push and pull request, cheap checks before
expensive ones, and a change is done when it is green; the local loop in
`CLAUDE.md` is its fast subset, never a substitute. The workflow is the list of
what runs, and is not restated here. What CI owes the rules:

- every suite against real backends — Postgres, the Redis and Valkey versions
  the docs claim, S3 — never a mock (`CLAUDE.md`, hard "no");
- the non-e2e suites under `NESTRS_ENV_PREFIX=ACME`, which is what holds *no
  env-var name spelled as a literal*;
- the demo, the docs lint (`docs.yml`, on a change to what the pages quote),
  and `cargo deny` over every lockfile.

The Redis/Valkey matrix and the feature matrix run nightly, on demand and on a
release branch, not on every pull request — a PR waits only on what its change
can break. Advisory lanes (beta clippy nightly, `cargo mutants` on a pull
request labelled `mutants`, minimal versions) report and never block. The workflows are hardened: actions pinned by SHA,
`persist-credentials: false`, least permissions, `zizmor` clean.

**The security watch is a monitor, not a gate.** It runs daily — the advisory
check and a beta-toolchain build — and opens or updates one issue on failure.
Nothing waits on it: it exists because CI runs only when someone pushes, and an
advisory is published whether or not anyone does.

## Release

The tag equals the workspace `version`; the procedure is in `publish.yml`'s
header. `CHANGELOG.md` follows Keep a Changelog, and a breaking change carries
its upgrading entry (`CLAUDE.md`, hard "no").
