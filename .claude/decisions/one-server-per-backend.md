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

**2026-11-06, the atomic migration's two sentences above corrected:** "the lease
is thirty seconds" is wrong: that test ran on the suite's seeded one-second
`LEASE`, at the soak's commit too, so a runner stalled a second between a delivery
and its settle lets the reclaim run the job again, which at-least-once allows, and
the soak's double count never had that cause ruled out. "Closes the first of the
two paths" is overstated: the push script's `held` reads `entries` and `delayed`,
which a job's end clears, so a push sent again after its job ended is filed as a
new job, as `JobProducer::enqueue` states and the queue kit holds. So the tests
moving a slot, and every test of the suite counting a job's runs and about no
lapse, now run on the default lease and the default budget
(`patient_replica_on`); a test about a lapse or a renewal, or counting no run, stays
on the second, and so do the queue kit's cases, whose one lease from their backend
several wait out. A `== 1` of the suite's own is then a promise that holds: a second
run left is a defect to find. Run on the
Cluster, a runner stalled 1.5 s at a time ran two jobs twice at the one-second lease
with the handler held 5 ms past its count, one reclaimed under the same `backend_id`
and one handed back after its attempt was cut, and ran none twice at the default
lease under the same stalls, so a lapsed lease is a cause of that count, though the
soak's own log is gone and the handler as committed did not double in 22 such runs.

**2026-11-06, where the demo's topology URLs sit:** the entry of 2026-10-08 says
`demo/.env`, and that file holds no Redis, since every profile reads it, production
included. Each topology's URL sits commented in `demo/.env.development`, under the
one server's `NESTRS_REDIS__URL` and `NESTRS_REDIS__TRANSCODES__URL`.

**2026-11-08, a stall of the runner under two seconds fails no count and no
administration command:** the entry of 2026-11-06 left a lapse rerunning a
job, which at-least-once allows, able to fail three kinds of test on a loaded
runner. A test about a lapse now forces it rather than waiting the one-second
lease out: the checkpoint through a dead replica runs on the default lease,
and the lost settle's answer on a renewed lease of three seconds, since its
worker retries the settle until the lease would lapse — on the default lease
that line came 25 s after the mute, a test of 26 s — and each lapses what is
held through `XCLAIM … IDLE` once its subject is in place. The queue kit's
cases hold the longer of two leases their backend offers (`KitLease::Lasting`)
and lapse only through `lapse` and `take`; the two whose subject is a renewal
hold `KitLease::Brief`, the same three seconds on Redis: a renewal goes a
third into a lease, so a stall under two seconds lapses nothing, where 1.5 s
stalls lapsed the one-second lease in each of 30 runs of the long attempt's
case, its job run 7 or 9 times, and none in 10 runs at three seconds. That
case holds its attempt half a lease past the lease, which still fails without
renewals; three leases made it the suite's slowest test, 10 s. Across a lapse
no test forced, no count is asserted. The suite's own connections — its
`CLIENT LIST`, `ACL`, `CLUSTER` and `MIGRATE`, its reads of what a test filed
and its probe of a user's rights — open and are answered within the
framework's default budget, where `redis`'s second to open failed the
slot-moving test three times in ten under 1.5 s stalls and none in ten after;
those telling a frozen node apart keep `redis`'s. Whether a lapse caused the
2026-10-14 soak's double count cannot be shown: its log is gone. The stalls of
2026-11-06 showed it is enough to make that count, and no test counting runs
can now meet one. What a stall may still fail, the behaviour right, each line
written in TODO.md: a test on the three-second lease — the kit's two renewal
cases and the lost settle — under a stall past two seconds, two thirds of it,
since a longer lease lengthens each of them with it; and a test counting no
run on the one-second lease, past about two thirds of it, which correlation,
trim and the entry deleted while delivered did not show in ten runs each
under 1.5 s stalls, the failovers and the KEDA confinement left undriven.

**2026-11-11, the lost settle waits for what Redis holds, not for its lease:**
the lost settle held the renewed lease of three seconds only because it waited
for its worker to give the settle up, at the lease's end, which it says at
`error` — the port's line, proven in process on paused time
(`a_settle_failing_past_its_lease_is_said_and_the_job_left_to_its_lease`), so a
second copy on Redis, and one a stall past two thirds of the lease fails. It
now runs on the queue's default lease and the suite's 500 ms budget, so each
lost answer fails its ask within the budget: the test waits for the worker to
say twice that it asks again — the settle and its ask again each ran on Redis
and lost its answer — then for the stream to empty, then lapses what is held
and looks: nothing it waits on follows a lease, and a stall lapses nothing
short of twenty seconds. The renewal is the one thing a test still waits out,
so the kit's two renewal cases keep the renewed lease. A stall past two thirds
of it fails each with the behaviour right — the worker wakes past its lease and
cuts its own attempt, whatever the backend: the kit over the in-memory backend
on real time with Redis's three-second lease failed 5 of 10 case runs under
2.5 s stalls and none under 1.5 s stalls, each stall driven by a nextest target
runner that starts the test as its child and stops it by its own PID from its
first instant.

**2026-11-11, the stalls driven on Redis, and the one a CI runner shows:** the
same runner drove each test on the dev container's Valkey. The kit's two
renewal cases failed 14 of 20 case runs under 2.5 s stalls and none of 20 under
1.5 s; the lost settle's earlier spelling, which waited for the give-up, failed
2 of 10 under 3.5 s stalls, and one waiting for the stream to empty on the
default budget none of the 11 runs a stall reached (14 stalls in 20 runs). The
queue page's confinement, on the one-second lease and the suite's 500 ms
budget, stayed green through 48 stalls of 1.5 s in ten runs: not reproduced, it
stays there. The failovers failed 7 of 40 runs under 1.5 s stalls and none of
11 bare, never on a count or a lapse: a boot or a push past the suite's budget,
and Sentinel refusing the forced failover, `INPROG` (their own failover begun,
past their two-second `down-after-milliseconds`) or `NOGOODSLAVE` (no replica
they would promote yet). Moving a failover's app to the default budget was
refused: its subject is the app leaving the frozen primary, which a call cut at
the suite's budget does within the freeze, where one on ten seconds would wait
the freeze out and write to the old primary as it wakes, before its demotion
discards the write. So the app keeps the budget, a boot or a push beside the
failover is asked again while refused, its job counted at least once, and the
forced failover is asked again each second until one is taken or another
primary is named. The delayed push and the cancel closing Sentinel's failover
keep one ask: none of the 24 stalled runs reaching them failed there. The
renewed lease stays three seconds: `just test` and a probe sleeping a
millisecond in a loop, both pinned to four CPUs as GitHub's hosted Linux runner
gives a public repository, saw a longest stall of 165 ms in 340 s, six past
20 ms and none past 250 ms — under a tenth of the two seconds the lease
tolerates, and under the 500 ms budget. The services ran on the container's
other CPUs, and a hosted VM's hypervisor steal is not measured there; the same
probe on a hosted runner is what closes it.
