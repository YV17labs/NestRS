_default:
    @just --list

# Run an app in watch mode (default: api). Usage: just dev live
dev app="api":
    bacon run-long -- --bin {{app}}

# Run an app in release mode (default: api). Usage: just run live
run app="api":
    cargo run --release --bin {{app}}

# Build one app in release (default: api). Usage: just build live
build app="api":
    cargo build --release -p {{app}}

# Build release binaries for every app in the workspace.
build-all:
    cargo build --workspace --release

# Database lifecycle — migrations + seeding. Usage: just db up|down|fresh|status|seed|reset
mod db

# Run unit + integration tests (no DB)
test:
    cargo nextest run --workspace -E 'not binary(e2e)'

# Run e2e tests (Postgres required)
test-e2e:
    cargo nextest run --workspace -E 'binary(e2e)'

# Run coverage on the full suite
test-cov:
    cargo llvm-cov nextest --workspace

# Clippy (strict) + format check
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all --check

# Apply rustfmt across the workspace
fmt:
    cargo fmt --all

# Fast type-check (no codegen)
check:
    cargo check --workspace
