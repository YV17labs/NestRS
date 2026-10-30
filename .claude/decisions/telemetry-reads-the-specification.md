# Telemetry reads the OpenTelemetry specification, under the binary's name

`nest-rs-opentelemetry` is configured by the variables the OpenTelemetry
specification names (`OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`,
`OTEL_EXPORTER_OTLP_*`, `OTEL_TRACES_SAMPLER` and `_ARG`,
`OTEL_METRIC_EXPORT_INTERVAL`), never by copies of them under
`<PREFIX>_OPENTELEMETRY__*`; and the service is named after the binary, whose
name and version `#[nest_rs::main]` reads off its manifest (`AppIdentity`).
Decided 2026-10-30, under the owner's agreement to the identity audit's
public API breaks ("Tout faire", 2026-10-28), which also exempts `OTEL_*`, as
`RUST_LOG` is, from the hard "no" on env-var literals.

Standards over invention: an operator, or the OpenTelemetry Operator injecting
`OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME`, got no export and a
service name the code chose, with no word said; one exporter was configured
from two namespaces at once (its timeouts under `OTEL_*`, the rest under ours).

- **The SDK reads what it reads, and nothing overrides it**: the resource
  (`OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`) and the sampler
  (`OTEL_TRACES_SAMPLER`). What `OpenTelemetryConfig` pins, and the binary's
  name and version, sit under them; the SDK's `unknown_service` fallback is
  replaced by the binary's name. One order is the specification's against
  the SDK's: `OTEL_SERVICE_NAME` wins over a `service.name` in
  `OTEL_RESOURCE_ATTRIBUTES`, which opentelemetry_sdk 0.33's detectors read
  the other way round, so the crate lays it last.
- **The rest is resolved by the crate, as the specification orders it** — a
  signal's own variable, then the general one, then the pinned field: the
  endpoints, the export timeouts, the metric period and the certificates, read
  through the SDK's constants where it exports them and the specification's
  names where it does not. Resolving them here is what lets a value be refused
  naming its variable and never quoted (the SDK's own errors quote the
  endpoint), held to its range (the metric period's second to an hour), and
  matched to the client this crate builds per signal (`https` alone unless the
  endpoint says `http://`, TLS material refused beside plaintext).
- **The certificates are the specification's files** —
  `OTEL_EXPORTER_OTLP_CERTIFICATE`, `_CLIENT_CERTIFICATE`, `_CLIENT_KEY`, per
  signal or for all three — read through the one bounded reader into a
  `ClientTls`, whose refusals name them through `nest_rs_config::TlsNames`.
- **A variable under the retired namespace is said, never read**: `init`
  names each `<PREFIX>_OPENTELEMETRY__*` it finds once at `warn`, since no
  config reads that namespace and the unclaimed-variable report would stay
  silent, while the deployment exported nothing.
- **`OpenTelemetry::init()` takes no argument**, and `OpenTelemetryConfig` is
  set one way: its public fields, over `Default` or `from_env`; the `with_*`
  setters and `new(name)` are gone.

**Refused:**

- **Installing telemetry from `#[nest_rs::main]` whenever the `opentelemetry`
  feature is linked** (correctness). Cargo unifies features across what one
  invocation builds, and the demo's workspace enables the feature for every
  app: a binary's telemetry would depend on whether it was built with
  `--workspace` or `-p`, the link-activated behaviour module-gating refuses.
- **An explicit `#[nest_rs::main(opentelemetry)]` key owning the guard**
  (elegance: one way to do a thing). It would still need
  `OpenTelemetry::init_with(config)` beside it for a pinned config — a key
  taking a value would be a second configuration grammar — so starting
  telemetry would have two spellings; `#[main]`'s grammar stays closed, and
  `init`'s error reaches the boot through the `?` the developer already
  writes. The trap it closed — `let _ = OpenTelemetry::init()?` dropping the
  guard at once — closes at the type's rung instead: the guard is
  `#[must_use]`, and `clippy::let_underscore_must_use` refuses the discard
  where it is on (the demo, the framework).
- **A reload slot filled by `OpenTelemetryModule::for_root(config)`.** The
  `reload` layer takes a lock on every span and event, which every app would
  pay whether it exports or not, and what is emitted before the module
  registers would reach the console only.
- **Keeping `<PREFIX>_OPENTELEMETRY__*` beside `OTEL_*`.** Two names for one
  setting, one of them invented: 7.0 is the major, so no alias.
