# tracing-opentelemetry

## A span's name recorded after it started is dropped

- **Seen** — 0.34.0: `on_record` updates a started span through
  `SpanBuilderUpdates::update_span` (`src/layer.rs:242`), which applies the
  status and the attributes and drops `otel.name`; a span starts as soon as its
  context is read (`OpenTelemetrySpanExt::context`).
- **Effect** — the framework reads the sampler's verdict off every unit's span as
  it opens, which starts it, so every HTTP request, named once the router
  matched, exported as `http.request`.
- **Our side** — `operation_log::record_name` records `otel.name` and calls the
  rename the linker seeds, `SpanRef::update_name`
  (`crates/nest-rs-opentelemetry/src/linker.rs`).
- **Canary** — `nest-rs-opentelemetry`'s integration
  `linker::upstream_tracing_opentelemetry_still_drops_a_name_recorded_on_a_started_span`.
- **Upstream** — not filed.
- **When fixed** — remove `rename` and its seed from `linker.rs`;
  `record_name` stays the edges' one call. Minutes.
- **Ticket** —

  > **`otel.name` recorded on a started span is ignored**
  >
  > Once a span's OpenTelemetry context exists (after `context()`, or on enter
  > with context activation), `on_record` applies `SpanBuilderUpdates` through
  > `update_span`, which sets the status and the attributes but drops `name`
  > (and `span_kind`, which the API cannot change). A span named late — an HTTP
  > server span named `{method} {route}` once the router matched — keeps its
  > static name. `SpanRef::update_name` exists for this; could `update_span`
  > call it when `otel.name` is recorded?

## An `error` event fails the span it is filed in, whatever that span's unit answered

- **Seen** — 0.34.0: `OpenTelemetryLayer::on_event` sets the status of the span
  an event is filed in to `Error("")` when the event's level is `ERROR` — in the
  `OtelDataState::Builder` arm while the status is unset, in the `Context` arm
  on every such event (`src/layer.rs`, around lines 1446 and 1463). No setting
  turns it off: `with_error_events_to_status` governs only an unnamed event's
  `error` field.
- **Effect** — a unit whose own line files `ok` exports `Error` when a framework
  `error` line was filed inside its span: a request whose handler rendered a
  `5xx` by hand (`HttpError::into_response` files its chain at once) and then
  answered `2xx`; a graphql-ws socket one of whose subscription items failed; an
  MCP operation whose tool handled a failure, whose chain is filed all the same.
  The span itself records no error and no description; the export says `Error`.
- **Our side** — none: a documented limit, in
  `.claude/decisions/client-error-is-its-line.md` (2026-11-14, *The limits*) and
  `docs/src/content/docs/opentelemetry/traces.mdx` (*A failed span says why*).
  Keeping the framework's `error` lines off the trace layer would drop every
  span event it writes, which is no fix of this.
- **Canary** — `nest-rs-opentelemetry`'s unit test
  `linker::tests::upstream_tracing_opentelemetry_still_fails_the_span_an_error_event_is_filed_in`.
- **Upstream** — not filed.
- **When fixed** — a setting turning it off: `init` sets it, and the limit
  leaves the decision and the page. An hour.
- **Ticket** —

  > **An `ERROR` event sets its span's status with no way to turn it off**
  >
  > `on_event` sets the status of the span an event is filed in to
  > `Status::error("")` whenever the event's level is `ERROR`, in both the
  > builder and the started arm. An instrumented service that logs a failure
  > it then recovers from — a fallback answered `200`, an error a handler logs
  > and handles — exports its span as failed, against the semantic conventions'
  > *Recording errors*, which leave the status to the operation's outcome.
  > `with_error_events_to_status(false)` covers only an unnamed event's `error`
  > field. Could the level mapping follow that setting, or one of its own?
