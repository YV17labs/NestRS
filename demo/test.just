# Test recipes, exposed as `nestrs run test <kind>` (see `mod test` in the root
# Justfile). Bare `nestrs run test` lists these — pick a kind to run.
# (e2e loads the project's `.env` from the harness, so no env wiring here.)

# Bare `nestrs run test` lists the kinds instead of running one.
_default:
    @just --list test

# Unit + integration + doctests, no DB.
unit:
    cargo nextest run --workspace -E 'not binary(e2e)'
    cargo test --workspace --doc          # nextest skips doctests; run them too

# e2e tests (live Postgres/Redis), gated by the nextest binary filter.
e2e:
    cargo nextest run --workspace -E 'binary(e2e)'

# Nothing to install by hand: `cargo-llvm-cov` comes with the CLI's first-run
# bootstrap, and the `llvm-cov` / `llvm-profdata` it shells out to come from the
# `llvm-tools-preview` component pinned in `rust-toolchain.toml`. A toolchain
# without rustup ignores that file entirely — set LLVM_COV / LLVM_PROFDATA there.
# `just --list` shows the line above a recipe, so the summary goes last.
# Coverage on the full suite (includes the e2e binary).
cov:
    cargo llvm-cov nextest --workspace

# Doctests only — the code examples inside `///` doc comments.
doc:
    cargo test --workspace --doc
