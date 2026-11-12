# A client operation is recorded by the driver that sent it, by what it does

Settled on 2026-11-12, with `unit-duration-from-its-span.md`, at the owner's
request to show inside a trace what a unit waited on — the statements and the
commands — as `@nestjs/observe` does for its drivers.

A driver times what it sends only while something records it
(`client_operation::is_recorded`), and hands a `ClientOperation` to the
recorder the observability stack seeds: `nest-rs-seaorm` through sea-orm's
`set_metric_callback`, which reports each statement once it answered;
`nest-rs-redis` around its one `ConnectionLike`. The stack turns each into a
`client` span with its exact start, under the current unit's span, and a point
in the histogram the conventions name.

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
- **The pool's connection gauges** (`db.client.connection.*`) in this change:
  an observable instrument needs a seam of its own from the driver, which
  lands when a pool's saturation is the next thing an operator asks.
