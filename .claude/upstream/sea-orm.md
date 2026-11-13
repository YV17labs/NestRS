# sea-orm

## Transaction control and unprepared text are not timed

- **Seen** — 2.0.4: the metric callback wraps `execute`, `query_one`,
  `query_all` and `stream`, never `execute_unprepared`
  (`src/driver/sqlx_postgres.rs:197`, `src/database/transaction.rs:474`),
  `begin` (`sqlx_postgres.rs:256`), `commit` or `rollback`
  (`transaction.rs:238`, `:295`).
- **Effect** — a transaction an app opens itself on the injected
  `DatabaseConnection`, and the text it sends through `execute_unprepared`,
  record no `db.client.operation.duration` and no client span.
- **Our side** — the driver times what it sends itself: a unit's `BEGIN`,
  `COMMIT` and `ROLLBACK`, an atomic block's `RELEASE` and `ROLLBACK TO`, the
  executor's `execute_unprepared` and the outbox relay's statements, through
  `Statements::sent` (`crates/nest-rs-seaorm/src/statements.rs`). The limit is
  on `opentelemetry/traces.mdx`, *Statements and commands*.
- **Canary** — `nest-rs-seaorm`'s e2e
  `statements::upstream_sea_orm_still_leaves_transaction_control_and_unprepared_text_untimed`.
- **Upstream** — not filed.
- **When fixed** — drop `Statements::sent` where sea-orm now times the call
  (`executor.rs`, `savepoint.rs`, `queue/outbox.rs`); the e2e tests asserting
  each statement once catch a double count. About an hour.
- **Ticket** —

  > **Metric callback does not cover `execute_unprepared`, `begin`, `commit` and `rollback`**
  >
  > `DatabaseConnection::set_metric_callback` is called for `execute`,
  > `query_one`, `query_all` and `stream`, but not for `execute_unprepared`
  > (`driver/sqlx_postgres.rs`, `database/transaction.rs`), nor for the
  > `BEGIN`, `COMMIT` and `ROLLBACK` a transaction sends. An application
  > timing its database through the callback misses its transaction control,
  > whose `COMMIT` is often the slowest statement (fsync, synchronous
  > replication). Could `metric!` wrap these calls too, with the statement
  > text sea-orm sends (`BEGIN`, `COMMIT`, `ROLLBACK`, the unprepared SQL)?

## A failed statement's error is not reported

- **Seen** — 2.0.4: `metric::Info` carries `elapsed`, `statement` and
  `failed: bool` (`src/metric.rs:11`), never the error.
- **Effect** — a statement sea-orm times records `error.type = _OTHER` where
  the conventions ask for the SQLSTATE (`40001`, `57014`), and no
  `db.response.status_code`.
- **Our side** — the statements the driver times record the SQLSTATE
  (`Refusal::of`, `statements.rs`); the others record `_OTHER`, said on
  `opentelemetry/traces.mdx`.
- **Canary** — none: the field's absence is a compile-time fact, read when the
  version moves.
- **Upstream** — not filed.
- **When fixed** — `Statements::record` reads the error through `Refusal::of`.
  A few lines.
- **Ticket** —

  > **Metric callback: expose the error of a failed statement**
  >
  > `metric::Info::failed` says a statement failed but not how. Exposing the
  > error (or at least the database's error code, the SQLSTATE on Postgres)
  > would let an application key its failures the way OpenTelemetry's
  > database conventions ask (`error.type`, `db.response.status_code`).

## A statement dropped before it answered is not reported

- **Seen** — 2.0.4: `metric!` calls the callback after the statement's future
  completed (`src/metric.rs:22-35`); a future dropped mid-flight reports
  nothing.
- **Effect** — a statement cut by a unit's deadline or a timeout around it, the
  slowest ones, never reaches the durations or the trace.
- **Our side** — the statements the driver times record a dropped one with no
  failure (`PendingClientOperation`); the limit is on
  `opentelemetry/traces.mdx`.
- **Canary** — none.
- **Upstream** — not filed.
- **When fixed** — `Statements::record` maps the cancellation to no failure.
  A few lines.
- **Ticket** —

  > **Metric callback: report a statement whose future was dropped**
  >
  > A statement cancelled by its caller (a `tokio::time::timeout`, a dropped
  > request) is never reported to the metric callback, since `metric!` reports
  > after the future completes. These are the slowest statements. A drop guard
  > reporting `elapsed` until the drop, with a flag saying it was cancelled,
  > would keep them in the metrics.

## A stream is reported without its failure, its start unknown

- **Seen** — 2.0.4: `MetricStream` reports from `Drop` with `failed: false`
  whatever its items were, and `elapsed` summed over its polls
  (`src/database/stream/metric.rs:77-88`).
- **Effect** — a stream an app reads itself records as succeeded, and its client
  span starts where the polling time, not the wall time, puts it. `Repo` never
  streams.
- **Our side** — the documented limit, on `opentelemetry/traces.mdx`.
- **Canary** — none.
- **Upstream** — not filed.
- **When fixed** — nothing: `Statements::record` already reads `failed` and
  starts the span `elapsed` before the report.
- **Ticket** —

  > **`MetricStream` reports `failed: false` and the summed poll time**
  >
  > `MetricStream`'s `Drop` reports `failed: false` even when the stream yielded
  > an `Err`, and `elapsed` as the sum of its poll durations. Could it record
  > whether any item was an error, and the wall time from its first poll to its
  > end?

## A transaction's `BEGIN` includes the wait for a connection

- **Seen** — 2.0.4: `SqlxPostgresPoolConnection::begin` acquires from the pool
  and sends `BEGIN` in one call (`src/driver/sqlx_postgres.rs:256-268`), and no
  public API begins a transaction on a connection already acquired.
- **Effect** — a unit's `BEGIN` duration counts the pool's wait, and
  `db.client.connection.wait_time` is not exported (see also `sqlx.md`).
- **Our side** — the driver's `BEGIN` is timed as the caller observes it, the
  wait included, said on `opentelemetry/traces.mdx`; an acquire timeout records
  `timeout`.
- **Canary** — none.
- **Upstream** — not filed.
- **When fixed** — time the acquire as `db.client.connection.wait_time` and the
  `BEGIN` alone. Half a day with the record path `sqlx.md` describes.
- **Ticket** —

  > **Begin a transaction on an acquired connection, or report the acquire time**
  >
  > `DatabaseConnection::begin` acquires a pooled connection and sends `BEGIN`
  > in one call, so an application cannot tell the pool's wait from the
  > statement. Either a way to begin on a connection already acquired, or the
  > acquire duration reported beside the transaction, would let it export the
  > wait as OpenTelemetry's `db.client.connection.wait_time`.
