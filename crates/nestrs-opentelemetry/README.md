# nestrs-opentelemetry

OpenTelemetry init + an HTTP interceptor for `traceparent` propagation,
per-request spans, status recording, the `X-Trace-Id` response header, and
one access event per request.

`OpenTelemetry::init("service-name")` reads `NESTRS_OPENTELEMETRY__*` and
installs the global `tracing` subscriber (console fmt always; OTLP exporters
when the `otlp` feature is on and `NESTRS_OPENTELEMETRY__OTLP_ENDPOINT` is
set). The returned guard flushes on drop and must outlive `main`.
`OpenTelemetryModule` mounts the per-request interceptor.

## Extending

The pluggability is already there via the underlying ecosystem — this crate
is one composition of `tracing` + `tracing-subscriber` + `opentelemetry`,
not a wrapper that hides them. Three orthogonal extension axes:

- **A different exporter** (Jaeger, Zipkin, Datadog Agent). Skip
  `OpenTelemetry::init` and install your own `tracing` subscriber with the
  exporter layer of your choice. Everything in nestrs emits through
  `tracing` targets (`nestrs::http`, `nestrs::orm`, …) — the subscriber is
  the seam.
- **Extra `tracing` layers** (sampling, structured-event sinks). Compose
  them into the subscriber you build instead of calling `OpenTelemetry::init`.
- **A different per-request interceptor** (your own span schema, your own
  propagator). Skip `OpenTelemetryModule` and register your own
  `Interceptor` from `nestrs-middleware`.

A community crate replacing the wiring would be named e.g.
`nestrs-datadog`. It exposes its own `<Name>Module` and `<Name>::init`
guard; an app picks one of `OpenTelemetryModule` *or* the alternative,
never both.
