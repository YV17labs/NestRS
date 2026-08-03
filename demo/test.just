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

# Coverage on the full suite (includes the e2e binary). The one recipe with a
# prerequisite beyond the CLI bootstrap: `cargo install cargo-llvm-cov` plus the
# LLVM tools it shells out to — `rustup component add llvm-tools-preview`. On a
# toolchain without rustup, set LLVM_COV / LLVM_PROFDATA instead.
cov:
    cargo llvm-cov nextest --workspace

# Doctests only — the code examples inside `///` doc comments.
doc:
    cargo test --workspace --doc
