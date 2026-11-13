# sqlx

## The pool counts no waiting caller and times no acquire

- **Seen** — 0.9.0: `Pool` exposes `size()` and `num_idle()`
  (`sqlx-core/src/pool/mod.rs:535`, `:540`) and its options; no count of the
  callers waiting for a connection, and its hooks (`after_connect`,
  `before_acquire`, `after_release`) carry no timing of the wait, the connect or
  the use.
- **Effect** — of OpenTelemetry's pool metrics, `db.client.connection.count`,
  `.max`, `.idle.min` and `.timeouts` are exported; `.pending_requests`,
  `.wait_time`, `.create_time` and `.use_time` are not.
- **Our side** — the documented limit, on `opentelemetry/metrics.mdx`, and the
  pool's reading in `crates/nest-rs-seaorm/src/pool.rs`.
- **Canary** — none: a new API is read in the release notes when the version
  moves.
- **Upstream** — not filed.
- **When fixed** — `pending_requests` is one more field of `PoolConnections`. The
  three durations are histograms recorded as they happen, so the pool needs a
  record path beside `nest_rs_core::connection_pool`'s readings, and possibly a
  pool built by the driver to hook it. About a day.
- **Ticket** —

  > **Pool metrics: waiting callers and acquire timing**
  >
  > `Pool` exposes its size and idle count, but not how many callers are
  > waiting for a connection nor how long an acquire waited, a connection took
  > to open, or was held. These are what OpenTelemetry's database pool
  > conventions name (`db.client.connection.pending_requests`, `.wait_time`,
  > `.create_time`, `.use_time`) and what an operator reads first when a pool
  > saturates. Would a `Pool::num_waiting()` and a hook (or a stats struct)
  > reporting acquire and connect durations be welcome?
