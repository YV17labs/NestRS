# The framework workspace. `demo/` and `bench/` are separate projects with their
# own Justfiles.

_default:
    @just --list

# Format every crate
fmt:
    cargo fmt --all

# Formatting and clippy
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Every test, against the dev container's Postgres, Redis and S3
test:
    cargo nextest run --workspace
    cargo test --workspace --doc

# rustdoc as docs.rs builds it
doc:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps

# Every check: lint, docs and tests
verify: lint doc test
