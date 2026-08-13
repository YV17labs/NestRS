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
  `unsafe_code`; every crate opts in with `[lints] workspace = true`.
  A new crate MUST carry that block. The few crates keeping
  source-level unsafe attrs are documented in the root manifest
  comment — don't add to them.
- **Third-party versions live in `[workspace.dependencies]` only**;
  member crates say `dep = { workspace = true }`. Some pins are
  **exact** (`=`) with a bump procedure documented in the root
  manifest comments — respect the procedure, never bump casually.
  A new dependency answers to the 12-month freshness bar
  (`CLAUDE.md` hard no).

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
- Intra-workspace dev-deps stay **path-only** (no `version`) so
  publishing doesn't drag test-only cycles.
- Product crates under `demo/` set `publish = false`; `demo/` is its
  own workspace and never joins the root `members`.
- `rust-toolchain.toml` pins the toolchain and matches the workspace
  `rust-version`; `.cargo/config.toml` (mold) is inherited by `demo/`
  hierarchically — never duplicated.

## CI is NOT the gate

`.github/workflows/` holds only `publish.yml` (tag `v*.*.*` →
`cargo workspaces publish`) and `docs-pages.yml` (docs lint + deploy).
**No CI runs clippy/fmt/nextest.** The *Definition of done* in
`CLAUDE.md` is enforced locally, by you, every time — never assume CI
will catch what you skipped.

## Release

The tag must equal the workspace `version`; the process lives in
`publish.yml`'s header comments. `CHANGELOG.md` follows
Keep-a-Changelog.
