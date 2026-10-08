# The framework workspace. `demo/` and `bench/` are separate projects with their
# own Justfiles.

_default:
    @just --list

# Format every crate
fmt:
    cargo fmt --all

# Formatting, the workflows' hardening, clippy, each capability alone, and the dependency policy
lint:
    cargo fmt --all --check
    cargo deny check bans licenses sources
    actionlint
    zizmor --offline --quiet .github
    cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings
    cargo hack check -p nest-rs -p nest-rs-macro-hygiene --each-feature --exclude-all-features --keep-going

# Each lockfile is read as committed: one its manifests outgrew fails rather than
# being resolved again. An ignore is judged in the framework's tree, where it is
# decided: a tree that does not reach the crate says nothing about it.
# Every Cargo lockfile the repository owns but the benchmarks' against RustSec, which moves without a change here
audit:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo deny --locked check advisories
    for lock in $(git ls-files '*/Cargo.lock' ':(exclude)bench/'); do
        cargo deny --locked --manifest-path "$(dirname "$lock")/Cargo.toml" check advisories --allow advisory-not-detected
    done

# Tests, one recipe per kind; `just test` runs them all
mod test

# rustdoc over the private items, then as docs.rs builds it: each pass refuses
# a broken link the other cannot see
doc:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going --document-private-items
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going

# Every check CI runs: lint, docs and tests
ci: lint doc test::all
