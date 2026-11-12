# A plaintext opening a deployment wrote is honoured and said

2026-11-06. Plaintext is allowed where the protocol's own URL declares it, and
the threat model lists each client's rule. Storage said its opening at `warn`;
Redis, Postgres and OTLP said nothing, so a `redis://` copied from a development
file into production ran in silence. Security orders it: an opening is an
explicit, visible line.

**What holds.** A deployed profile (`Environment::is_deployed`, an undeclared one
included) files one `warn` on the crate's target where the resource opens,
naming the endpoint without what its URL carries, and connects all the same:

| Driver | Where | Once per | Says it for |
|---|---|---|---|
| Redis | `RedisConnection::open`, before any topology dials | instance | a URL of no TLS, but one server on the loopback or a socket |
| Postgres | the pool's `connect`, the app's and `connect_from_env`, never a connection a tool opens itself from `SeaOrmConfig::connect_options` | pool | a resolved mode of `disable`, off the loopback and off a socket |
| OTLP | after `init` installs the subscriber | signal | an `http://` endpoint off the loopback |
| Storage | `Store::open` (already) | store | `ALLOW_HTTP` open beside an `http://` endpoint, on any host |

The four sentences keep one shape, `<subject> outside a dev profile: <what>
travel unencrypted`, each on its crate's target; a fifth driver takes the same
shape. Held by each driver's unit tests.

- **Redis says it at the open, not in `tls::material`.** `material` runs at every
  client rebuild, on all three topologies, so it would repeat a line that belongs
  to the instance.
- **Postgres reads the resolved mode, not the URL's text.** sqlx-postgres reads
  libpq's `PGSSLMODE`, so `PGSSLMODE=disable` beside a URL naming no mode turns
  TLS off as the parameter does. Honoured as libpq's own standard variable, and
  said by the same line.
- **Postgres says it where the framework opens, not where options are built.**
  `SeaOrmConfig::connect_options` opens nothing, and a tool may build them and
  never connect, so a connection a tool opens itself from them says nothing: the
  open is the tool's, and so is what it says of it. A limit named, not a gap in
  the framework's own opens.
- **A socket is what sqlx dials as one**: a URL's `host=`, or a host starting with
  `/`, where `PGHOST` and sqlx's default put theirs.

**Forks, decided by the values:**

- **The loopback and a socket say nothing, on Redis and Postgres as on OTLP.** It
  was asked of OTLP alone, a collector beside the app being the common shape. The
  reason holds whole for a Cloud SQL proxy, a PgBouncer or an Envoy sidecar, where
  `sslmode=disable` to `127.0.0.1` is correct and nothing crosses a network; a line
  on correct configuration teaches an operator to filter the target, which would
  hide the line that matters.
- **A Sentinel or Cluster URL says it whatever its hosts.** Those links reach a
  primary and nodes the URL does not name, so its hosts on the loopback prove
  nothing about where the traffic goes. Security outranks a quiet line on a shape
  no production runs.
- **Storage keeps saying it on the loopback.** Its trigger is `ALLOW_HTTP`, a
  setting a deployment opens on purpose, closed by default, whose sentence also
  covers presigned URLs handed outside the process. It is no gap, and moving it
  would drop a line nobody reported as noise.

**Refused:**

- **Refusing a plaintext URL in a deployed profile.** The protocol's URL declares
  it and a server without TLS exists; the framework still connects to what a
  deployment names (`dev-services-tls.md`).
- **Refusing `PGSSLMODE`.** libpq's variable is the standard's spelling, and every
  other mode it can name is refused as the URL's is, but `prefer`, sqlx's own
  default, which reads as no mode named and connects with `verify-full`.
- **A `warn` per reopened connection or per pool connection.** The opening is one
  fact; the line would fill a log during an outage.
- **A setting that silences the line.** An opening is visible by rule; closing it
  is writing `rediss://`, `sslmode=verify-full` or `https://`.

## 2026-11-08 — Postgres says it where the options are built

The owner's mandate of 2026-11-07 closes the limit the entry named: a connection a
tool or a test opened itself from `SeaOrmConfig::connect_options` said nothing, so a
migration tool reaching production in plaintext was the one opening no line named.
The line moves into `connect_options`, the one constructor every Postgres opening
goes through — the app's pool, `connect_from_env`, `EphemeralDatabase` and a tool's
own — and leaves the pool's `connect`, so each opening built says it once. A caller
building options it never connects with reads the line too: the deployment wrote
plaintext, and that is what the line states. **Refused:** a public opening function
beside `connect_options`, a second way to open what the constructor already builds.
