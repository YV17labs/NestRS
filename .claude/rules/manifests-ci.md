---
paths:
  - "Cargo.toml"
  - "crates/*/Cargo.toml"
  - "demo/**/Cargo.toml"
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
