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
Each environment also sets `NESTRS_REDIS__URL` on the dev container to its
topology, and `test.just` reads the topology off it, so the everyday
`just test` runs on whichever environment is open — a `just test topology`
recipe, which only those environments needed, was dropped on the owner's
word: the open environment decides, never a command.
Then the environments set nothing: a variable on the dev container outranks
every `.env`, so it moved the demo's suites off the database 1 `.env.test`
gives them, and they refused to run. Each topology's URL sits commented in
`test.just`, for the framework's suites, and in `demo/.env`, for the demo's
apps — one line to uncomment, the default the one server — and the demo's
suites stay on `.env.test`'s, at the owner's word.
Then every node is a host of the compose network, reached by its name like
`valkey-standalone`, at the owner's word: a name that only the one server
carried was no convention. The nodes had shared the dev container's loopback,
so a topology's URL named `127.0.0.1` and a port per node; each now listens on
Valkey's port and announces its name — the Cluster's preferred endpoint, every
address Sentinel reports — which the certificate names, one row per node. A
production deployment announces names too, so the suites now prove the
framework follows them. The Sentinel servers are numbered, never named for the
role a failover swaps. A Cluster node keeps nothing across a start, which may
move every peer's address, so each start forms the Cluster afresh, and no slot
a test left migrating outlives it.

**2026-10-12, the known flake's refusal closed:** `CLUSTER MIGRATESLOTS`
refused was the Cluster suite's three tests that move a slot meeting each
other, and it failed two runs in a row on a 7.0 branch that changed neither
the suite nor the binding. Valkey excludes the two kinds of migration both
ways, on every node either touches: a node holding a slot in a manual
`SETSLOT … MIGRATING` or `IMPORTING` refuses to start or to accept an atomic
migration, and a node running an atomic one refuses every `SETSLOT`, the
`SETSLOT NODE` a test sends every primary to hand a slot over included. Each
of those tests touches two of the three primaries, so any two can refuse each
other; a retry around the one refusal seen would have left the others. They
now take turns on a file lock, since nextest runs each test in a process of
its own. The next run found a second cause, the atomic migration's alone:
Valkey fails every atomic migration a node runs when any of its databases is
flushed ("Data was flushed"), and a Cluster client sends `FLUSHDB` to every
primary, so a test emptying a database of its own beside it left the slot
with its source. Every flush of the suite now goes through one helper holding
the same lock shared, for the command alone: flushes overlap one another,
never a test moving a slot. A retry of the migration, Valkey's own remedy,
was refused: each later flush would reach it again. The other failure
recorded above, a job counted twice in CI, is not one this race produces —
a collision ends in a refusal or a migration that failed whole, never a
second delivery — and stays as recorded.

**2026-10-14, a client certificate proved live on every shape:** every
Valkey of the dev container runs `tls-auth-clients no`, so the refusal of a
client presenting no certificate was proved against in-process doubles
only. A test of a `mutual_tls` module now asks every data node of the
Sentinel and Cluster environments for one (`CONFIG SET tls-auth-clients
yes`) over a connection opened before, which keeps serving, and gives each
node its own setting back whatever the test did; the suite runs the module
last and alone, beside the failovers, since every other test's connection
would be refused meanwhile. Presenting the certificate boots and reaches
every primary; without it the boot fails at once, at the Cluster's seed, at
the primary the sentinels name, and at that primary reached as one server —
the standalone proof, run in the Sentinel environment because CI's one
server is shared by every test of its run. A sentinel takes no `CONFIG
SET`, so the sentinels' own refusal stays proved in process. The dev
container presents the services' certificate, which carries `clientAuth`,
from a copy of its own (`issue-certificates.sh`'s clients), installed from
the kept certificate so adding it issues nothing. Refused: a node of each
topology requiring a certificate from its start — a new hostname, so a new
certificate, which reissues the authority under every running service.

**2026-10-14, the Cluster suite soaked:** twenty Cluster runs in a row, the
slot lock and the flush helper in place, ran
`a_queue_runs_on_through_an_atomic_migration_of_its_slot` green each time,
every job counted once, so the double count recorded above did not recur and
its cause stays unfound. It is not the redelivery of a lapsed lease: the lease
is thirty seconds, and the test stops its worker once every job has run,
waiting twenty at most, so no delivery lapses inside it. Two paths can still
run a job twice there: a push whose answer was lost, which the cluster client
sends again (`redis` retries a command after a dropped or timed-out
connection, and the script files a second entry for the same job), and a
delivery the move loses on its way to the target, which would be Valkey's. The
idempotent enqueue the queue port takes next closes the first, so the test
keeps asserting once: a second run is a defect to find, never a redelivery to
allow. The same pass showed a failover's freeze outliving its test: the frozen
node slept past the Sentinel failover's return, and the `mutual_tls` test
after it found that node unanswering. A test freezing a node now waits, within
twice the freeze, until it answers again before returning.

**2026-10-15, the atomic migration re-run after the idempotent enqueue:** the
push script now skips a job its queue still holds, the contract
`JobProducer::enqueue` states for every driver, which closes the first of the
two paths above: a push the cluster client sends again after its answer was
lost files nothing twice. `a_queue_runs_on_through_an_atomic_migration_of_its_slot`
ran green on the Cluster environment after it, every job counted once, beside
the whole Redis suite on the Sentinel and Cluster environments. The test keeps
asserting once: a second run left would be a delivery the slot's move lost,
Valkey's, and is still a defect to find.

**2026-10-17, a Postgres requiring a client certificate is an environment of
its own:** the dev container's Postgres and CI's sign in by password, so the
client certificate sqlx presents had no live proof. A `cert` line in the
everyday `pg_hba.conf` would need `ssl_ca_file` on the server's command line:
the running server would refuse the reload, and once restarted without being
recreated, refuse to start. So `postgres-mutual-tls` layers on `base.yml`,
reconfigures the one `postgres` service — same hostname, so the certificate
and its authority stay as issued — on a data volume of its own, and lets
`nestrs_certificate` in on the dev container's certificate alone
(`pg_ident.conf`), every other role keeping its password. `nest-rs-seaorm`'s
`mutual_tls` tests present it and omit it; `test.just` leaves them out unless
its `postgres` line names that environment. Refused: a second Postgres
service under a new hostname, for the reissue named above; `ALTER SYSTEM` or a
role created on the everyday volume, which is the developer's data.
