_default:
    @just --list

# Run an app in watch mode (default: api). Usage: nestrs run dev live
dev app="api":
    bacon run-long -- --bin {{app}}

# Run an app in release mode (default: api). Usage: nestrs run start live
start app="api":
    cargo run --release --bin {{app}}

# Build in release: one app (default api), or every app with `--all`.
# Usage: nestrs run build live   |   nestrs run build --all
build app="api":
    cargo build --release {{ if app == "--all" { "--workspace" } else { "-p " + app } }}

# Database lifecycle — migrations + seeding. Usage: nestrs run db up|down|fresh|status|seed|reset
mod db

# Tests — unit/integration/e2e/coverage/doctests. Usage: nestrs run test [e2e|cov|doc]
mod test

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
