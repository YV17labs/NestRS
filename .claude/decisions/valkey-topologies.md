# Valkey's three topologies, declared by the URL's scheme

`nest-rs-redis` reached one server — a primary, or a name the DNS moved at a
failover — while `CLAUDE.md` asks a supported backend be supported whole: every
deployment shape its latest release offers in production, TLS on each. Valkey
9.1 offers three ("Deployment topology" in its docs): one server, Sentinel,
Cluster. The owner decided on 2026-10-08 that every binding runs on all three,
on these terms.

- **The scheme declares the topology**, in the grammar fred's `Config::from_url`
  documents — `redis[s]-sentinel://`, `redis[s]-cluster://`, further hosts as
  `node=` parameters, `sentinelServiceName`, `sentinelUsername`,
  `sentinelPassword` — so the URL stays RFC 3986 and two clients read one
  secret alike. The one departure: the path's database is honoured on a
  Cluster, which Valkey 9 serves (`cluster-databases`). `RedisConfig` and the
  variables do not change; `RedisTopology` names the three.
- **Sentinel follows Valkey's Sentinel client spec**, written here over the
  `redis` client's connections: `SENTINEL GET-MASTER-ADDR-BY-NAME`, the role of
  the address named, and every reconnection asking again.
- **What a connection reached is read off its `HELLO`**, which Valkey lets
  every user send: its `mode` refuses at once a URL declaring another topology,
  on all three, and its `role` proves the primary the sentinels name where the
  spec says `ROLE`, which every role's rule would have had to grant. Matching
  the text of a refused `CLUSTER` or `SENTINEL` is what it replaced.
- **Cluster is `redis`'s cluster connection**, its nodes opened through a
  connection of ours, which skips `CLIENT SETINFO` (an ACL-confined user is
  refused it) and says a node's TLS refusal; the seam is its doc-hidden
  generic connection, until `redis` offers the setting — so `redis` is pinned
  `=1.7`, as `async-graphql` is for its registry internals. A script's keys sit
  in one slot, checked before it is sent on every topology.
- **A blocking read on a Cluster reaches its slot's primary alone**, a socket
  per drained method, found through `CLUSTER SLOTS`; it follows an `ASK` and is
  opened afresh by its holder after a `MOVED` or a loss. A second cluster
  connection held one socket to every node, replicas included, per method.
- **KEDA's rules are what its client sends**, recorded against KEDA 2.21 and its
  go-redis 9.22 on each topology and replayed by the suite as the pages print
  them; KEDA's cluster scaler reads database 0 alone, which the pages and the
  demo chart say.
- **Test isolation stays the logical database** on every topology, the test
  Cluster running `cluster-databases 16`.
- **The dev container keeps one server** (owner, 2026-10-08): Sentinel and
  Cluster run in CI's test job from the `topology-services` action, on the
  runner's loopback, after `just test` and on its build (`just test topology
  sentinel|cluster`). The one certificate gained `clientAuth`, which a
  Cluster's bus asks of every node.

**Refused:**

- **Lettuce's comma-separated hosts** (`host1,host2` in the authority): not an
  RFC 3986 URL. **Variables per topology** (`SENTINEL_URLS`, `CLUSTER_URLS`): a
  second way to say the URL, and combinations to refuse.
- **`redis`'s `sentinel` feature**: it discovers through `SENTINEL MASTERS`,
  waits without a bound, and its `ConnectionManager` reopens the address it
  had, which after a failover is a replica.
- **Inferring the topology** from what a server answers: a deployment declares
  it, as a recurring job declares where it fires; `HELLO` checks the
  declaration, never replaces it.
- **Reading from replicas, following `+switch-master`, a node address map,
  `valkey://` schemes**: a lagging copy under a script; an optional channel
  that guarantees nothing a reconnection does not; Valkey announces hostnames
  itself; `decisions/valkey-only.md`.
- **Sentinel and Cluster in the dev container**, and a certificate per service
  or per topology (`decisions/dev-services-tls.md`).

Reopened when `redis` gives a Cluster a setting to skip `CLIENT SETINFO`, or
reconnects through Sentinel itself as the spec asks — then the seam it replaces
goes, and the pin with it — and when KEDA moves to another client, whose
commands its rules follow.

**Moved on 2026-10-08, the same day** (`one-server-per-backend.md`): CI runs
the one server alone, and Sentinel and Cluster leave the CI job for the dev
container's `topologies` profile — the `topology-services` action's services
and its formation script, unchanged but for their network, the dev container's
own, so the nodes still announce `127.0.0.1`, which the certificate names.
`just test topology <name>` sets the profile's URL itself. The refusal of
Sentinel and Cluster in the dev container above stands for its default
services; a profile starts nothing unless asked for.
Then two dev container environments rather than a profile, `valkey-sentinel/`
and `valkey-cluster/` (`one-server-per-backend.md`).
