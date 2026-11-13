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
- **The pool's connection gauges** (`db.client.connection.*`) in this change:
  an observable instrument needs a seam of its own from the driver, which
  lands when a pool's saturation is the next thing an operator asks.
