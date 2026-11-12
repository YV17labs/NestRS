# CI runs on a change, never on a clock

On 2026-10-08 the owner audited what CI ran on `release/7.0` and kept only what
keeps the product from regressing, run when a change can turn it red:
`Framework` (lint, every test), `Demo` (its lint and suites), `Docs` (the build,
the deploy from `main`), `Audit` on a change to a Cargo tree, and `Publish` on a
tag.

**Refused:**

- **The benchmarks in CI.** `bench.yml` linted them on every framework change,
  recompiling the framework once more per push, and `just audit` read their
  lockfiles, so a bench lagging the framework's manifests turned both red with
  no defect in the product. The benchmarks are run, linted and kept up to date
  by a developer, on their host (`bench/RUNBOOK.md`); a bench that breaks waits
  for its next run.
- **A scheduled run.** `beta.yml` checked both workspaces on the beta toolchain
  weekly, and `audit.yml` ran daily on `main`. Neither had ever run: a schedule
  reads the default branch's workflows, and `main` did not carry them. A rustc
  release breaking a crate is now met when the pin moves, or by a user on the
  new stable; an advisory against a tree nobody changes is met at its next
  change or by `publish.yml`, which refuses to ship it. Dependabot alerts watch
  `main` without a run — a repository setting, the owner's.
- **A tool or a service in a job's name.** `test (Postgres, Redis, S3)` grew
  with every backend and still said Redis once the server was Valkey;
  `lint (fmt, clippy, rustdoc)` named three of the seven tools it ran. A job
  names what it runs, and parentheses carry a closed set: the test kinds.
- **`CI` as a workflow's name**: every workflow is CI. A workflow is named for
  the tree a change to it triggers, which its `paths` already state.
- **Two docs workflows.** `docs.yml` built the site on a change and
  `docs-pages.yml` built it again on `main` to deploy it; one workflow builds,
  and deploys from `main` alone.
- **The whole docs tree in `Framework`'s paths.** Every page edit ran the
  framework's lint and tests for the three pages nest-rs-redis's e2e reads;
  those three are listed, and a test that reads another page adds it.
- **A cache saved by `main` alone** while work lives on `release/**` for weeks:
  every release run rebuilt from a stale cache. rust-cache saves on every
  branch by default; the long-lived branches save, a pull request restores its
  base's.

## 2026-10-22 — the audit runs on a clock again, and a change answers only for what it brings

RUSTSEC-2026-0381, 0382 and 0383 were published against versions every
lockfile already held, and `Audit` turned pull request #4 red: it moved
`Cargo.lock` by one unrelated line, and `just audit` failed on every advisory
in the tree, wherever it came from. An alert is wanted; a pipeline that stops
every change until a third party ships a fix is not. The owner reversed the
refused scheduled run for the audit alone: an advisory is the one red that
comes from a clock, not a change.

- A pull request runs `just audit <base>`, which fails only on a diagnostic
  absent at the base — the same advisory on the same crate and version, per
  lockfile, the base judged by its own `deny.toml` — and warns of one already
  there.
- A daily run, and `workflow_dispatch`, check the default branch and every
  `release/**` head whole, one job per branch: red alerts and blocks nobody.
  Its fix is a Dependabot security update, with version updates off, or a bump
  by hand on a release branch, which Dependabot never targets.
- `publish.yml` keeps the whole check: a release ships no advisory nobody
  decided, its exit a bump or an `ignore` with its reason.
- The push trigger went: a protected branch receives its changes through a
  checked pull request.

**Refused:**

- **GitHub's dependency review** for the pull request: it reads
  vulnerabilities alone, while `deny.toml` also refuses unsound, unmaintained
  and yanked crates.
- **A diff of the lockfiles** in place of a run at each end: it misses a change
  to `deny.toml` — an ignore removed, a stale one added — which turns the check
  red as surely as a version does.
- **The pull request's check left non-blocking**: it would let through the one
  advisory a change can settle, its own.
- **A second recipe**: `just audit` takes the base as an optional argument.

## 2026-10-28 — the audit leaves the clock again: Dependabot watches the default branch

Asked what CI the 7.0 run had added and what it cost, the owner removed the
daily audit: GitHub's Dependabot already alerts on the lockfiles'
vulnerabilities, keeping a clock over them is GitHub's job, not the project's,
and dropping it saves two minutes of CI a day. The run had never started
either: a schedule reads the default branch's workflows, and `main` did not
carry `audit.yml` yet.

- `audit.yml` runs on a pull request touching a Cargo tree, and on nothing
  else: `just audit <base>` refuses an advisory the change brings, as the
  2026-10-22 entry decided.
- The schedule and `workflow_dispatch` are gone, with the two jobs only they
  ran: the listing of the default branch and every `release/**` head, and the
  whole check on each.
- `publish.yml` keeps the whole check before a release.
- Dependabot's alerts and security updates, a repository setting, watch the
  default branch's lockfiles for the vulnerabilities GitHub's advisory database
  holds; `dependabot.yml` still opens security updates alone, version updates
  off.

**No longer watched**, between a tree's changes and its publish:

- a `release/**` branch's lockfiles: Dependabot reads the default branch alone;
- on every branch, the default one included, what `deny.toml` refuses beyond
  that database: an unmaintained crate, a yanked version, and an unsound crate
  GitHub has not imported. Dependabot, like the dependency review the
  2026-10-22 entry refused, reads GitHub's advisory database alone: in the
  RustSec database of 2026-10-27, 6 of its 283 unmaintained advisories and 160
  of its 228 unsound ones carry a GitHub alias, and a yanked version is no
  advisory at all.

Either surfaces at the next pull request touching that tree, as a warning since
the base holds it, and at `publish.yml`'s whole check, which refuses the release
until a bump or an `ignore` with its reason settles it.

**Refused:** `workflow_dispatch` kept for a whole check by hand. `just audit`
runs it on any checkout, and the pull request's job reads a merge commit's
first parent, which a dispatched run does not have.

## 2026-11-11 — the vital minimum: no bench, image or Windows job

The second pass (B-230, 2026-11-08) added `bench.yml`, a `demo.yml` `image` job on
release branches and tags, and a `framework.yml` `windows` job, to close `TODO.md`
lines that asked for proof of the Dockerfiles and of Windows. The owner, on
2026-11-11: "on doit économiser la CI … c'est le minimum vital". All three are
removed, and so is the chart's render job: the owner, the same day, keeps the demo's
Helm chart out of every run but a developer's (`nestrs run chart`).

**Refused, again:** the benchmarks in CI, any image build (the demo's, a
contestant's) and a platform the product does not claim (Windows). Each costs
runner minutes on every push it runs on and turns nothing red that a product
defect would; a Dockerfile is built when its owner runs it (`bench/RUNBOOK.md`, the
release check), and Windows stays a limit named in `TODO.md`. A new check is a step
of an existing job before it is a job of its own.
