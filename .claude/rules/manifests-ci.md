---
paths:
  - "Cargo.toml"
  - "crates/*/Cargo.toml"
  - "demo/**/Cargo.toml"
  - "bench/**/Cargo.toml"
  - "rust-toolchain.toml"
  - ".cargo/**"
  - ".github/**"
  - "CHANGELOG.md"
---

# Manifests, CI & release

## Workspace manifests (root = framework)

- **Lints are workspace policy.** `[workspace.lints]` forbids
  `unsafe_code` and warns on `unreachable_pub` — the mechanical half of
  `architecture.md`'s *`pub` means exported*: an item no re-export in
  `lib.rs` reaches is `pub(crate)` or private, so a crate's public
  surface is its export list and nothing beside it. Every crate opts in
  with `[lints] workspace = true`; a new crate MUST carry that block.
  The few crates keeping source-level unsafe attrs are documented in the
  root manifest comment, and restate every other workspace lint in a
  `[lints]` block of their own — don't add to them.
- **Third-party versions live in `[workspace.dependencies]` only**;
  member crates say `dep = { workspace = true }`. Some pins are
  **exact** (`=`) with a bump procedure documented in the root
  manifest comments — respect the procedure, never bump casually.
  A new dependency answers to the 12-month freshness bar
  (`CLAUDE.md` hard no). **An existing one that crosses the bar is
  flagged at its pin, never kept silently:** `apalis` / `apalis-redis`
  0.7.4 is the one 7.0 keeps past it, with the evidence (the 1.0 RC
  line fails dead-replica recovery) and the condition to move written
  in the root manifest. No fork, no vendoring, no `[patch]` of a
  third-party crate: an upstream defect is reported upstream.

### `major.minor` — the one requirement form

**Every third-party requirement is spelled with exactly two
components** (`"1.53"`, `"0.14"`, `"=2.0"`), in every manifest the repo
owns: root, `demo/`, `bench/`, and the manifests the CLI *generates*
(`src/templates/`, `src/commands/generate/cargo.rs`).

- **The minor is the floor we actually build against** — the version
  the lockfile resolved. A bare major (`"1"`) claims less than we know:
  it accepts a 1.0 that never compiled here.
- **The patch belongs to the publisher.** Pinning it (`"1.53.1"`)
  rejects exactly the fixes a caret range exists to inherit.
- It is the form the CLI already derives for `nest-rs-*`
  (`version::framework_req`) — third parties now match it.

**Bumping the minor is part of `cargo update`**: when the lock moves a
minor, the requirement moves with it in the same change. Majors do not
move that way — the pinned-major policy in the root manifest freezes
several of them for the whole 1.x line, and any other major bump is an
owner decision (`CLAUDE.md`, *stop and ask*). A `cargo update` that
reports `available: vX` semver-incompatible releases is **reported, not
taken**.

**One exception, documented at its pin:** `async-graphql` /
`async-graphql-poem` carry `=7.2.1` because `nest-rs-graphql` reads that
crate's public-but-internal registry API. Nothing else carries a patch.

**The framework's own crates require each other at `=` the release** — every
`nest-rs-*` entry of the root `[workspace.dependencies]` is `"=7.0.0"`, and a
framework crate links a sibling only through `{ workspace = true }`. This is not
a third-party requirement, so the two-component form does not bind it; it is the
lockstep one tag publishes, stated where cargo reads it.

The reason is what an expansion calls. A `*-macros` crate emits calls into
`#[doc(hidden)]` seams that semver does not cover — its runtime crate's, and up
to eleven others' (`#[operations]` reaches `nest-rs-authz`, `-seaorm`, `-pipes`,
`-guards`) — through code `nest-rs-codegen` writes. Under a `"7.0"` floor a
partial `cargo update -p` could pair one crate's 7.0.1 expansion with another's
7.0.0 seams, and the error lands inside a macro expansion, blamed on the
attribute. The `serde` / `serde_derive` pin — a runtime requiring its macros
crate at `=` — closes one edge of that and leaves the codegen and every
cross-crate seam open, so the pin is the whole framework's. It costs a consumer
nothing: they require `nest-rs = "7.0"`, and the umbrella moves every crate at
once. Bump the requirements with `[workspace.package] version`.
`the_framework_requires_itself_at_its_own_release` (same file as
`versions_are_major_minor`) fails on a requirement that is not `=` the release,
and on a member spelling its own.

`versions_are_major_minor`
(`crates/nest-rs-cli/src/commands/generate/cargo.rs`) walks the repo's
manifests **and the ones the CLI generates** — the templates' raw-string
manifests and this file's own `workspace_value` literals, *discovered* from
the CLI's sources rather than listed, so a template added later is covered
the day it is written. It lives in the generator's suite because a
scaffolded workspace inherits these pins verbatim, and it reached only the
repo's three manifests for a while: the generated half was conformant, and
a drift there would have shipped to every new project without failing a
single suite here.

### One `nest-rs*` line per consumer

**A manifest that consumes the framework names the umbrella and nothing
else.** A second `nest-rs-*` line is the defect *The umbrella is the
front door* describes, not a local shortcut.
`consumers_name_only_the_umbrella` (same file) walks every consumer the
repo owns — `demo/` and each of its members, `bench/sut/nestrs`, and
`nest-rs-macro-hygiene` — and fails naming the offending crate.

`bench/sut/nestrs` is on that list because it is the one consumer
**outside both workspaces**: it carries its own empty `[workspace]`
table, so `cargo clippy --workspace` never reaches it and it drifted
back to a five-crate stanza unobserved. Anything else added outside the
workspaces inherits the same blind spot and belongs on the list the day
it is created.
- Intra-workspace dev-deps stay **path-only** — `{ path = "../nest-rs-x" }`,
  no `version` and no `workspace = true`, which carries one — so publishing
  doesn't drag test-only cycles, and a dev-edge to a crate published later
  never falls back to an index that lacks it. Cargo strips a versionless
  dev-dependency from the published manifest. One rule for every member,
  the unpublished `nest-rs-conformance` included; the dev-dependency half of
  `the_framework_requires_itself_at_its_own_release` holds it.
- Product crates under `demo/` set `publish = false`; `demo/` is its
  own workspace and never joins the root `members`.
- **The Rust floor is one value, restated everywhere it is read.**
  `rust-toolchain.toml` pins the channel and is the anchor; the three workspace
  `rust-version`s, the images' `ARG RUST_VERSION` and their `FROM rust:` tag,
  the publish workflow's toolchain, `MIN_RUST_VERSION` in `nestrs doctor` and
  the documented requirement all restate it, and everything `nestrs new`
  scaffolds restates it again. `toolchain_pins_agree` (same file as
  `versions_are_major_minor`, for the same reason) enforces **four**
  obligations, and each exists because the three before it were silent
  somewhere:
  - the **anchor** is a bare `major.minor` — unchecked, a `channel = "stable"`
    reported every correctly pinned site as stale and named whichever sorted
    first;
  - every value **agrees** with it, the site named in the failure;
  - every shape is **present**, since a scan that stops matching finds nothing
    and finding nothing reads exactly like finding nothing wrong;
  - and every value a *syntactic* marker introduces is **well formed**. A
    marker like `rust-version = "` or `channel = "` is syntax: what follows it
    *is* the floor, so `"1"`, `"1.96.1"`, `1.96-slim` and `"stable"` are stale
    or unpinned rather than prose, and are reported. Only the three English
    markers (`**Rust `, `pins Rust `, `` `rustc` ≥ ``) may legitimately open a
    sentence carrying no version, and only they are skipped.

  Presence is per shape, so it cannot see a pin deleted from one of several
  files sharing one. `every_workspace_root_declares_the_floor` is the
  obligation that can, and it is **derived, never listed**: a manifest that
  roots a workspace declares `rust-version`, and a member inherits it with
  `rust-version.workspace = true` — a root's floor that no member opts into is
  a value cargo never reads. Counting the shapes in prose is not one of the
  obligations, and deliberately: a hand-written numeral restating what the
  table already says is the same drift these rules exist to catch.

  It walks the templates **raw** rather than through `generated_manifests`,
  which keeps only TOML declaring dependencies: the scaffold's own
  `rust-toolchain.toml` declares none and its Dockerfile is not TOML, so the two
  pins a developer inherits most directly were invisible to the generated-half
  machinery. A benchmark's recorded `rustc` is a **measurement**, not a pin, and
  never moves with the floor.
- `.cargo/config.toml` (mold) is inherited by `demo/` hierarchically — never
  duplicated.

## CI is NOT the gate

`.github/workflows/` holds `publish.yml` (tag `v*.*.*` →
`cargo workspaces publish`), `docs-pages.yml` (docs lint + deploy) and
`security-watch.yml` (daily, and on a lockfile or manifest change on
`main`: cargo-audit over the three lockfiles with warnings denied, a build
on the Rust beta toolchain, and the **feature matrix** — every crate
checked alone under its own defaults, then the umbrella with each feature
alone; a failure opens or updates one issue).
**No CI runs clippy/fmt/nextest.** The *Definition of done* in
`CLAUDE.md` is enforced locally, by you, every time — never assume CI
will catch what you skipped.

**Every local build is one feature union**, and that is a blind spot of
its own: `--workspace` unifies every member's features, and the hygiene
witness enables all of them. A crate that compiles only because a sibling
turned a feature on passes both — `nest-rs-authz`'s 7.0 engine, before its
release, named the optional `nest-rs-core` from always-compiled code, so `authz` alone
and every headless `seaorm` build failed. The local half of the gate is the
`dependencies` join in `nest-rs-conformance`: every path rooted at an
**optional** dependency sits below a `#[cfg(feature = …)]` whose feature
enables it — through the `mod` tree, the item, the statement and a macro
call's tokens. What a static read cannot see — a path that resolves only
under a dependency's *forwarded* feature — is the feature matrix's.

The watch is a **monitor, not a gate**: nothing waits on it and it is
never a required check. It exists because the local loop cannot notice
an advisory published while nobody touches the repo — RUSTSEC-2026-0285
sat in all three lockfiles for eleven days. Every advisory it accepts is
argued in `.cargo/audit.toml`, one reason per ignore.

## Release

The tag must equal the workspace `version`; the process lives in
`publish.yml`'s header comments. `CHANGELOG.md` follows
Keep-a-Changelog.
