# The checks CI runs, runnable here. `just pre-commit` is the minute before a
# commit; `just ci` is every check CI runs, before a branch reaches the owner's.
# A check CI runs that cannot run here is a defect of the environment.

set shell := ["bash", "-euo", "pipefail", "-c"]

_default:
    @just --list

# Formatting, clippy and the in-process suites, without the compile-fail snapshots
pre-commit: fmt-check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo nextest run --workspace -E '!binary(e2e) & !test(/_diagnostics$/)'

# Every check CI runs, cheapest first
ci: lint supply-chain workflows test e2e features docs demo

# Apply rustfmt to every workspace
fmt:
    cargo fmt --all
    cd demo && cargo fmt --all
    cd bench/sut/nestrs && cargo fmt --all

# Check rustfmt on every workspace
fmt-check:
    cargo fmt --all --check
    cd demo && cargo fmt --all --check
    cd bench/sut/nestrs && cargo fmt --all --check

# Formatting, clippy on both workspaces, and rustdoc as docs.rs renders each crate
lint: fmt-check
    cargo clippy --workspace --all-targets -- -D warnings
    cd demo && cargo clippy --workspace --all-targets -- -D warnings
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps

# cargo-deny over every lockfile the repository owns, and cargo-machete
supply-chain:
    cargo deny --manifest-path Cargo.toml check
    cargo deny --manifest-path demo/Cargo.toml check
    cargo deny --manifest-path bench/sut/nestrs/Cargo.toml check --allow advisory-not-detected --allow license-not-encountered
    cargo machete

# actionlint and zizmor over the workflows
workflows:
    actionlint -color
    zizmor --offline .github

# The in-process suites with every snapshot, the doctests, and the suites again under another variable prefix
test:
    cargo nextest run --workspace -E '!binary(e2e)'
    cargo test --workspace --doc
    NESTRS_ENV_PREFIX=ACME cargo nextest run --workspace -E '!binary(e2e)'

# The live-backend suites
e2e: backends
    cargo nextest run --workspace -E 'binary(e2e)'

# The Redis adapter's live suite alone, as CI runs it against each server line
e2e-redis: backends
    cargo nextest run -p nest-rs-redis -E 'binary(e2e)'

# Each crate alone, under each of its features
features:
    cargo hack check --workspace --each-feature --no-dev-deps

# The docs site build, whose link validator fails on a dead internal link
docs:
    cd docs && { [ -d node_modules ] || npm ci; } && npm run build

# The demo's lint, unit and e2e suites, driven as a developer drives them
demo: backends
    cd demo && nestrs run lint && nestrs run test unit && nestrs run test e2e

# Stop before the live suites when a backend they need does not answer
[private]
backends:
    #!/usr/bin/env bash
    set -euo pipefail
    for url in "${NESTRS_SEAORM__URL:-postgres://postgres:5432}" "${NESTRS_REDIS__URL:-redis://redis:6379}" "${NESTRS_STORAGE__ENDPOINT:-http://rustfs:9000}"; do
      address=${url#*://}; address=${address#*@}; address=${address%%/*}
      host=${address%:*}; port=${address##*:}
      if ! timeout 3 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null; then
        echo "$host:$port does not answer: start the devcontainer's services before the live suites" >&2
        exit 1
      fi
    done
