# The workflows' hardening is held by zizmor and actionlint

`manifests-ci.md` asked that the workflows stay hardened — actions pinned by
SHA, `persist-credentials: false`, least permissions — and only review held
it. On 2026-10-06 the owner chose both tools, run by `just lint` and so by
CI's lint job on every change under `.github/`:

- **zizmor 1.30, offline audits** (`--offline`): the security half — template
  injection, a token persisted by checkout, excessive permissions, an action
  not pinned by SHA, `GITHUB_ENV` written from an input. Offline so that a
  developer holding a `GH_TOKEN` runs what CI runs; the online audits read
  databases that move without a change here, which is `just audit`'s kind of
  check, and are not run yet.
- **actionlint 1.7.12**: the correctness half — workflow syntax, expression
  types, and shellcheck over every `run:`. install-action does not carry it,
  so CI fetches the release by version and checks its SHA-256; shellcheck is
  the runner's and the dev container's (apt).

Their first run: actionlint rejected every `uses: $/…`, a syntax it predates
(`self-repository-actions.md`), so `.github/actionlint.yaml` ignores that one
sentence for a path under `.github/actions`; shellcheck flagged `beta.yml`'s
unquoted `$FEATURES`, now `${FEATURES:+"$FEATURES"}`; zizmor flagged
`setup-rust` writing its `toolchain` input to `GITHUB_ENV`. That write stays —
without it rust-cache keys beta's artifacts on the pinned toolchain — behind a
check refusing anything but a channel's or a version's characters, and a
`# zizmor: ignore[github-env]` beside the reason.

On 2026-10-08 `beta.yml` left CI (`ci-on-change.md`), and with it
`setup-rust`'s `toolchain` input, its `GITHUB_ENV` write and the ignore beside
it.
