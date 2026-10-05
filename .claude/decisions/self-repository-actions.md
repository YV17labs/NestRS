# A local action is referenced `$/`, never `./`

`uses: $/.github/actions/setup-rust` is GitHub's self-repository syntax
(changelog, 2026-07-30): it resolves to the workflow's own repository at the
commit that is running, with no checkout, on runner 2.336.0 or newer, on
github.com only. `./` loads the action from the checked-out workspace, so a
step that checked out another ref runs that ref's action; zizmor's
`self-repository` audit (1.30) flags it.

It flipped twice on 2026-10-03 — 089bf532 moved to `./` as "the only form GitHub
resolves", cbada2c8 reverted it — and was then left as never run on GitHub.
Checked on 2026-10-05 against a real run: astral-sh/uv's run 36775797384
(2026-09-30, commit ba14b136), job `fix-bug / fix` on a GitHub-hosted
ubuntu-24.04 runner, runs two steps `uses: $/.github/actions/<name>` — composite
actions, the form `setup-rust` takes — and both succeeded; job-level
`uses: $/.github/workflows/…` runs green in uv's `ci.yml` on every push. This
repository's own first run follows the owner's push.

Refused: `./`, the weaker form (the workspace's state, not the running commit),
proposed on a premise the run disproves. GitHub Enterprise Server lacks `$/`;
nestrs runs on github.com.
