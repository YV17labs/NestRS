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
