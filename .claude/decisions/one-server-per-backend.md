# CI runs each backend on one server; every other shape is run by hand

Decided by the owner on 2026-10-08. The e2e of every backend shape a supported
backend offers — Valkey's Sentinel and Cluster today, a clustered broker
tomorrow — would grow CI and the dev container with each backend the framework
adds, past what the owner's resources hold. So:

- **CI starts the dev container's default services and nothing else** —
  Postgres, Valkey and S3, one server each — and runs every test against them.
- **Every other shape keeps its e2e suite**, and its services sit in
  `.devcontainer/docker-compose.yml` under a compose profile, started by hand
  from the host (`--profile topologies` for Valkey). A developer runs it when
  the driver changes, and to reproduce an issue a user opens.
- **A new backend does the same**: its one-server form joins the default
  services only when CI must run it; a server needed to build a driver, and no
  more, sits under a profile.

`CLAUDE.md` still asks a supported backend be supported whole; this is how it
is proved: one shape on every change, the others when someone changes or
questions them. The owner accepts the risk that a regression on Sentinel or
Cluster surfaces only then.

**Refused:**

- **Every shape in CI**, on every change: each backend added multiplies the
  job, for shapes most changes never touch.
- **Every shape before a release** (`publish.yml`): one more run of every
  topology to keep in working order, at the moment a release is wanted.
- **Commented-out services** in the compose file: nothing parses a comment, so
  it rots unseen; a profile is validated by `docker compose config` and starts
  with one flag.

Known at the decision: `cluster::a_queue_runs_on_through_an_atomic_migration_of_its_slot`
is flaky. It failed in CI's last Cluster run (a job counted twice) and once by
hand (`CLUSTER MIGRATESLOTS` refused, the suite running beside it), and passes
alone; it is reproduced when an issue asks.

Reopened when the owner's resources allow a shape in CI again.

**Moved on 2026-10-08, the same day, at the owner's request:** the profile
asked a command on the host, which a contributor inside the dev container
cannot run, and services commented out were tried next and dropped for what
the devcontainer specification already offers: several configurations in one
repository, each an ordered list of compose files, picked with *Reopen in
Container* ("Connect to multiple containers", VS Code). The everyday
environment is `.devcontainer/devcontainer.json`, the services CI runs; each
backend shape a driver is built against is a folder beside it,
`<backend>-<shape>/` — `valkey-sentinel/`, `valkey-cluster/` — whose
`devcontainer.json` layers `compose/<backend>-<shape>.yml` on
`compose/base.yml`. The folder holds the configurations alone: every compose
file sits in `compose/`, every script in `scripts/` — the two lifecycle
commands too, so a configuration's copy carries no logic — and service
configuration in `config/`. Only the open
environment runs (`shutdownAction` stops a compose environment with its
window). Every Valkey service extends `compose/valkey.yml`, the one image
tag, and the standalone server is named for its shape, `valkey-standalone`,
like its siblings. A configuration inherits nothing from another (the
specification has no `extends`), so each environment copies the base's
`devcontainer.json` but for its name and its compose files.
