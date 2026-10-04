# The framework workspace. `demo/` and `bench/` are separate projects with their
# own Justfiles.

_default:
    @just --list

# Format every crate
fmt:
    cargo fmt --all

# Formatting, clippy, each capability alone, and the dependency policy
lint:
    cargo fmt --all --check
    cargo deny check bans licenses sources
    cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings
    cargo hack check -p nest-rs -p nest-rs-macro-hygiene --each-feature --exclude-all-features --keep-going

# The tree against the advisory database, which moves without a change here
audit:
    cargo deny check advisories

# Tests, one recipe per kind; `just test` runs them all
mod test

# rustdoc over the private items, then as docs.rs builds it: each pass refuses
# a broken link the other cannot see
doc:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going --document-private-items
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going

# Every check CI runs: lint, docs and tests
ci: lint doc test::all
