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
  in `nest-rs-macro-hygiene` and a `reason` naming what to use instead.
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
`"0.14"`, `"=2.0"`), in every manifest the repo owns — root, `demo/`, `bench/`
— and every manifest the CLI generates.

- **The minor is the floor we build against**, the version the lockfile
  resolved. A bare major (`"1"`) accepts a release that never compiled here.
- **The patch is the publisher's.** `"1.53.1"` refuses the fixes a caret range
  exists to inherit.

**The minor moves with `cargo update`**, in the same change as the lock. A
major never moves that way: the pinned majors listed in the root manifest are
the public surface the macros emit, so their bump ships in a nestrs major, and
any other semver-incompatible release is reported, never taken.

**One exception, at its pin:** `async-graphql` / `async-graphql-poem` carry a
patch, because `nest-rs-graphql` reads that crate's public-but-internal
registry API.

Held by `versions_are_major_minor` in `nest-rs-cli`, which walks the repo's
manifests and the ones the CLI writes — it lives in the generator's suite
because a scaffold inherits these pins verbatim — and by review.

### The framework requires itself in lockstep

**Every `nest-rs-*` entry of the root `[workspace.dependencies]` is `=` the
release**, and a framework crate links a sibling only through
`{ workspace = true }`. This is not a third-party requirement, so the
two-component form does not bind it. A `*-macros` crate emits calls into
`#[doc(hidden)]` seams semver does not cover, so a partial `cargo update -p`
under a `"7.0"` floor could pair one crate's expansion with another's seams and
fail inside a macro expansion. It costs a consumer nothing: they require
`nest-rs = "7.0"`, and the umbrella moves every crate at once. Bump the
requirements with `[workspace.package] version`. Held by
`the_framework_requires_itself_at_its_own_release` in `nest-rs-cli`.

**Intra-workspace dev-dependencies are path-only** —
`{ path = "../nest-rs-x" }`, no `version`, no `workspace = true` — so
publishing drags no test-only cycle and a dev-edge never falls back to an
index that lacks the crate. Cargo strips them from the published manifest.

### One `nest-rs*` line per consumer

**A manifest that consumes the framework names the umbrella and nothing
else** (`CLAUDE.md`, *The umbrella is the front door*). Held by
`consumers_name_only_the_umbrella` in `nest-rs-cli`, which walks every
consumer the repo owns: `demo/` and its members, `nest-rs-macro-hygiene`, and
`bench/sut/nestrs`. The last carries its own empty `[workspace]`, so no
`--workspace` command reaches it; anything else created outside both
workspaces has the same blind spot and joins that list the day it exists.

### Workspaces

- Product crates under `demo/` set `publish = false`; `demo/` is its own
  workspace and never joins the root `members`.
- `.cargo/config.toml` (the mold linker) is inherited by `demo/`
  hierarchically — never duplicated.

## The Rust floor is one value

`rust-toolchain.toml` pins the channel as a bare `major.minor` and is the
anchor. Every other site restates it — each workspace's `rust-version` (members
inherit it with `rust-version.workspace = true`), the images' `RUST_VERSION`
and `FROM rust:` tag, the publish workflow, `nestrs doctor`'s floor, the
documented requirement, and everything `nestrs new` scaffolds. Moving the
floor moves every site in one change. Held by `toolchain_pins_agree` and
`every_workspace_root_declares_the_floor` in `nest-rs-cli`. A benchmark's
recorded `rustc` is a measurement, not a pin, and never moves with the floor.

## Feature isolation and advisories

**Every local build is one feature union.** `--workspace` unifies every
member's features and the hygiene witness enables all of them, so a crate that
compiles only because a sibling turned a feature on passes both.
`scripts/check-features.sh` is the build that sees it: every crate alone under
each of its features, then the umbrella with each feature alone. It is a
tier-3 step in `CLAUDE.md`.

**Every ignore in `.cargo/audit.toml` is argued**, one reason per advisory,
and revisited on every dependency bump; how `cargo audit` runs is `CLAUDE.md`'s
tier 3.

## CI

`.github/workflows/` holds `publish.yml` (a `v*.*.*` tag publishes),
`docs-pages.yml` (docs lint and deploy) and `security-watch.yml`. **Tier 3 of
the Definition of done runs locally, by you**: no workflow runs clippy, fmt or
nextest, so never assume CI catches what you skipped.

**The security watch is a monitor, not a gate.** It runs daily and on a
manifest or lockfile change on `main` — the advisory audit, a beta-toolchain
build and the feature matrix — and opens or updates one issue on failure.
Nothing waits on it and it is never a required check; it exists because the
local loop cannot notice an advisory published while nobody touches the repo.

## Release

The tag equals the workspace `version`; the procedure is in `publish.yml`'s
header. `CHANGELOG.md` follows Keep a Changelog, and a breaking change carries
its upgrading entry (`CLAUDE.md`, hard "no").
