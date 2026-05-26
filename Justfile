_default:
    @just --list

# Run an app with hot reload (default: app). Usage: just dev mcp
dev app="app":
    bacon run -- --bin {{app}}

# Run an app in release mode (default: app). Usage: just run mcp
run app="app":
    cargo run --release --bin {{app}}

# Build release binaries for every app in the workspace
build:
    cargo build --workspace --release

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
