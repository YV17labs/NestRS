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
