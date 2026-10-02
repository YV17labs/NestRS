#!/usr/bin/env bash
# Every framework crate compiles alone: under its default features, under none,
# and under each of its features on its own.
#
# Every other build unifies features — `--workspace` enables the union of what
# every member asks for — so a crate that compiles only because a sibling
# turned a feature on is invisible there; `nest-rs-authz` shipped not compiling
# alone that way. Here each `cargo check -p` resolves the one crate's features
# and nothing else, so a path rooted at an optional dependency outside a gate
# that enables it, or a re-export gated on the wrong feature, fails under the
# feature that exposes it. `nest-rs-macro-hygiene` mirrors the umbrella's
# decorator capabilities one feature each, so the same loop proves each
# capability pulls what its own decorators emit.
#
# Crates and features are read from `cargo metadata`, never listed: one added
# later is checked the day it lands. `default` and `full` are unions, which is
# what every other build already is. Exits non-zero naming every combination
# that fails; under GitHub Actions each result is also written to the job
# summary.
set -uo pipefail

cd "$(dirname "$0")/.."

meta=$(cargo metadata --no-deps --format-version 1) || exit 1
status=0

summary() {
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        echo "$1" >>"$GITHUB_STEP_SUMMARY"
    fi
}

check() {
    local label=$1
    shift
    if cargo check --quiet "$@"; then
        echo "ok    $label"
        summary "- \`$label\`: ok"
    else
        echo "FAIL  $label"
        summary "- \`$label\`: **does not compile**"
        status=1
    fi
}

for crate in $(jq -r '.packages[].name' <<<"$meta" | sort); do
    check "$crate" -p "$crate"
    features=$(jq -r --arg crate "$crate" \
        '.packages[] | select(.name == $crate) | .features | keys[] | select(. != "default" and . != "full")' \
        <<<"$meta")
    [ -z "$features" ] && continue
    check "$crate with no features" -p "$crate" --no-default-features
    for feature in $features; do
        check "$crate with $feature alone" -p "$crate" --no-default-features --features "$feature"
    done
done

exit $status
