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
    cargo check -p nest-rs-macro-hygiene --no-default-features --features queue,testing

# Each lockfile is read as committed: one its manifests outgrew fails rather than
# being resolved again. An ignore is judged in the framework's tree, where it is
# decided: a tree that does not reach the crate says nothing about it. Against a
# base, each tree is judged at the base by the base's own deny.toml, and read
# after the change's: an advisory published between the two reads lands at the
# base, never on the change.
# Every Cargo lockfile the repository owns but the benchmarks' against RustSec; given a base revision, fails only on what the tree adds to it
audit $base="":
    #!/usr/bin/env bash
    set -euo pipefail
    locks=$(git ls-files Cargo.lock '*/Cargo.lock' ':(exclude)bench/')
    deny() {
        local manifest=$1/${2%Cargo.lock}Cargo.toml allow=
        [ "$2" = Cargo.lock ] || allow=advisory-not-detected
        shift 2
        cargo deny --locked "$@" --manifest-path "$manifest" check advisories ${allow:+--allow "$allow"}
    }
    status=0
    if [ -z "$base" ]; then
        for lock in $locks; do deny . "$lock" || status=1; done
        exit "$status"
    fi
    completed() {
        jq -R -e 'fromjson? | select(.type == "summary")' "$1" > /dev/null
    }
    # One line per error or warning a JSON run printed: its severity, what it
    # names (the advisory and the crate it hits), and its title.
    diagnostics() {
        jq -R -r 'fromjson? | select(.type == "diagnostic") | .fields
            | select(.severity == "error" or .severity == "warning")
            | .graphs[0].Krate as $krate
            | [.severity,
               if .advisory then "\(.advisory.id) (\(.code)) in \($krate.name) \($krate.version)"
               elif $krate then "\(.code) \($krate.name) \($krate.version)"
               else "\(.code) \(.labels[0].span)" end,
               .message]
            | @tsv' "$1"
    }
    report() {
        if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::$1 file=$2::$3"; else echo "$1: $2: $3"; fi
    }
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    mkdir "$work/base"
    git archive "$base" | tar -x -C "$work/base"
    for lock in $locks; do
        deny . "$lock" --format json 2> "$work/head.json" || true
        if ! completed "$work/head.json"; then
            jq -R -r 'fromjson? | select(.type == "log" and .fields.level == "ERROR") | .fields.message' "$work/head.json"
            report error "$lock" "cargo-deny could not read this tree"
            status=1
            continue
        fi
        : > "$work/known"
        if [ -f "$work/base/$lock" ]; then
            deny "$work/base" "$lock" --format json 2> "$work/base.json" || true
            completed "$work/base.json" || report warning "$lock" "cargo-deny could not read this tree at $base: each advisory counts as the change's"
            diagnostics "$work/base.json" | awk -F '\t' '$1 == "error" { print $2 }' > "$work/known"
        fi
        while IFS=$'\t' read -r severity subject title; do
            if [ "$severity" = warning ]; then
                report warning "$lock" "$subject: $title"
            elif grep -qxF "$subject" "$work/known"; then
                report warning "$lock" "$subject, already at $base: $title"
            else
                report error "$lock" "$subject, absent at $base: $title"
                status=1
            fi
        done < <(diagnostics "$work/head.json")
    done
    if [ "$status" -eq 0 ]; then
        echo "advisories ok: none absent at $base"
    else
        echo "An error above is this change's to settle: an advisory by the crate's bump, or by an ignore in deny.toml with its reason. \`just audit\` shows what pulls a crate in."
    fi
    exit "$status"

# Tests, one recipe per kind; `just test` runs them all
mod test

# rustdoc over the private items, then as docs.rs builds it: each pass refuses
# a broken link the other cannot see
doc:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going --document-private-items
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going

# Every check CI runs: lint, docs and tests
ci: lint doc test::all
