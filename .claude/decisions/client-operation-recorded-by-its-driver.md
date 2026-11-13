# A client operation is recorded by the driver that sent it, by what it does

Settled on 2026-11-12, with `unit-duration-from-its-span.md`, at the owner's
request to show inside a trace what a unit waited on — the statements and the
commands — as `@nestjs/observe` does for its drivers.

A driver times what it sends only while something records it
(`client_operation::is_recorded`), and hands a `ClientOperation` to the
recorder the observability stack seeds: `nest-rs-seaorm` through sea-orm's
`set_metric_callback`, which reports each statement once it answered;
`nest-rs-redis` around its one `ConnectionLike`. The stack turns each into a
`client` span with its exact start, at its place in the unit's trace, and a
point in the histogram the conventions name.

**The unit is the kernel's correlation, never the `tracing` span entered**
(2026-11-13): a task spawned under a request's scope runs the unit with no span
entered, and a root span (`parent: None`) carries a trace of its own, so either
made the client span disagree with the `traceparent` a client had announced. An
operation carries its own child correlation (`ClientOperation::correlated`), or
the recorder makes one off the unit it is recorded in.

**A client times an operation it may not see end through a
`PendingClientOperation`** (2026-11-13), which records on drop: an app's
`timeout`, a `select!`, the framework's net or a unit cut at its deadline drop
the call's future, and a client that had announced the operation's span to its
peer left that peer a parent never exported. A dropped operation records no
`error.type` and no status: the HTTP conventions (v1.42, *HTTP client span*)
say a cancellation by the caller "SHOULD NOT be treated as an error: the span
status SHOULD be left unset and `error.type` SHOULD NOT be set". The duration
it records is the time until the caller gave up, so the metric and the trace
count the same operations.

**Every HTTP request is one primitive, `HttpClientRequest`, in the kernel**
(2026-11-13): an app's reqwest client through `ReqwestMiddleware`
(`nest-rs-reqwest`), the JWK Set fetch, the OAuth client and object storage all
announce the request's span — `traceparent` and the unit's `tracestate`, never
`baggage` — and record it in `http.client.request.duration`, keyed by the method,
the server, the status and the HTTP version. A peer outside the deployment (an
identity provider, S3) is told the trace too: its ids are random, and the
`tracestate` forwarded is the one a trusted proxy handed in, which W3C Trace
Context asks every participant to forward. The OTLP export is the one client
left out: an export recorded would be exported in turn.

Refused:

- **A `tracing` span per statement, opened by the driver**: sea-orm reports a
  statement after it ran, and a `tracing` span cannot be backdated; a span per
  `Repo` method would name a call, not the statements it sent.
- **`db.query.text`**: a statement written by hand (`Statement::from_string`)
  may carry a value in its text, and no lexer short of Postgres's own strips
  every literal. The summary — the operation and the first table, read with
  literals, comments and quoted text skipped — is what is exported.
- **A span for an operation outside every traced unit**: a worker's poll and a
  relay's claim would each be a root trace; their durations still count.
- **`url.full` on an HTTP request's span**, which the conventions require: they
  redact credentials in the authority and, by default, the query values of
  `X-Amz-Signature`, `X-Amz-Credential`, `X-Amz-Security-Token`, `sig` and
  `X-Goog-Signature` — a list "subject to change over time". A webhook's token
  sits in its path and an app's own signed parameter under any name, which no
  list reaches, and a span is kept by a backend no one audits for them. The span
  carries the scheme, the server and the port, and nothing of the path.
- **One series per host, cut down**: `server.address` keys the duration, as the
  conventions require it to, so a client calling hosts its users choose keys one
  series per host. Said on `HttpClientRequest` and the docs page, rather than
  bounded by a set no app declares.
- **A summary guessed past what the lexer reads with certainty**: the
  summary is lexed as Postgres's own scanner reads SQL (every quoting of a
  literal, nested comments, dollar-quoted bodies, quoted identifiers), and a
  statement it cannot read with certainty — a quote never closed, a backslash
  in a standard string, whose meaning `standard_conforming_strings` decides
  on the server — has none, named `postgresql`: a guessed word could be a
  value.

## 2026-11-13 — what sea-orm sends untimed, and the pool

sea-orm 2.0's callback times `execute`, `query_one`, `query_all` and a
stream, never `execute_unprepared`, `begin`, `commit` or `rollback`. The
driver times those it sends itself through one path, `Statements::sent`:
a unit's `BEGIN`, `COMMIT` and `ROLLBACK`, an atomic block's settle, the
outbox relay's statements and the executor's `execute_unprepared`. The
conventions are silent on transaction control
(open-telemetry/semantic-conventions#3695; Java's JDBC instrumentation records
commit and rollback), so each is the client operation its general rules make
it, summarised `COMMIT`. A `BEGIN` includes the wait for its pooled
connection, which sea-orm acquires inside the same call: the only place a
unit's wait for the pool shows, said in the docs.

The pool's connections reach the export through a kernel seam,
`nest_rs_core::connection_pool`: a driver implements `ConnectionPool` and
`observe`s it, the registry holding it weakly so what an app opened goes with
it, and `nest-rs-opentelemetry` reads every one still open at each export.
`nest-rs-seaorm`'s observation lives in the statement callback the pool's
handle holds, so it lasts as long as the pool. The instruments are the
conventions' 1.42 names (`db.client.connection.max`, renamed `.limit` in 1.45
with `db.system.name`, `db.namespace` and `server.*` added), the schema the
export declares; they move with the semantic-conventions crate.

Refused:

- **Recording the boot proof and the health probe**: they are the framework
  asking, not the app's work, and a probe every few seconds would weigh on the
  durations an operator reads for the app's statements.
- **Counting acquire timeouts where each acquire happens**: sea-orm acquires
  inside `execute`, `begin` and the rest, and the driver sees the failure only
  as the `DbErr` it returns. `RepoError::from`, where the data layer classifies
  every failure it hands out once, counts it; one binary holds one pool, so the
  count is the pool's. A timeout an app meets on the injected connection and
  never converts is not counted.
- **What sqlx does not tell**: `pending_requests` (no waiter count),
  `wait_time`, `create_time` and `use_time` (no acquire or connect timing),
  `idle.max` (sqlx bounds idle connections by `max_connections` alone). Each
  needs a hook upstream, in sqlx's pool or sea-orm's connection.
- **Correcting a stream's report**: sea-orm reports a stream from its `Drop`,
  with the time spent polling it as its duration and `failed: false` even when
  it yielded an error, through the same callback as a statement, so the driver
  cannot tell it apart. `Repo` never streams; an app streaming on its own
  connection gets sea-orm's report. Fixing it is sea-orm's: a `MetricStream`
  that records its wall time from the first poll and whether an item was an
  error.
