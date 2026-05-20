_default:
    @just --list

# Run the API with hot reload (rebuild + restart on file change)
dev:
    bacon run -- --bin api

# Run the API in release mode
run:
    cargo run --release --bin api

# Build the release binary
build:
    cargo build --release --bin api

# Run the full test suite (parallel, fast)
test:
    cargo nextest run --workspace

# Test coverage summary (text, per-file)
cov:
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
