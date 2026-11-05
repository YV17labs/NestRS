# The framework workspace. `demo/` and `bench/` are separate projects with their
# own Justfiles.

# Warnings fail the each-capability checks in every crate they build. A `cfg` every
# target matches joins `.cargo/config.toml`'s rustflags; `RUSTFLAGS` would replace them.
deny_warnings := "--config=target.'cfg(all())'.rustflags=['-D','warnings']"

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
    cargo hack check -p nest-rs -p nest-rs-macro-hygiene --each-feature --exclude-all-features --keep-going "{{deny_warnings}}"
    cargo check -p nest-rs-macro-hygiene --no-default-features --features queue,testing "{{deny_warnings}}"
    cargo check -p nest-rs-macro-hygiene --no-default-features --features authn,testing "{{deny_warnings}}"

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
# a broken link the other cannot see. rustdoc renders a link into a hidden
# `__private` module dead and without a warning, as the bare path
# (`href="crate::__private::…"`), and a trait method's `crate::` link the same on
# an implementor in another crate, so the public pass renders alone and no page
# of it may link there. A third party's trait on a blanket impl renders its own
# (tracing's `Span`, itertools', tokio's `io`), which is not ours to write. A page
# prints a seam in a signature with no link, so what `#[expose]` emits on a row is
# held by its bound's name (`src`, the pages of rustdoc's source view, is excluded).
doc:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going --document-private-items
    rm -rf "${CARGO_TARGET_DIR:-target}/doc"
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --keep-going
    test -f "${CARGO_TARGET_DIR:-target}/doc/nest_rs/index.html"
    ! grep -rhoE 'href="([^"]*__private[^"]*|crate::[^"]*)"' "${CARGO_TARGET_DIR:-target}/doc" --include='*.html' | grep -vE '"crate::(Span|Itertools::[a-z_]+|io::[A-Za-z]+)"'
    ! grep -rl 'AModelCrossesNoWire' "${CARGO_TARGET_DIR:-target}/doc" --include='*.html' --exclude-dir=src

# Every check CI runs: lint, docs and tests
ci: lint doc test::all
