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
  guard at once — closes at the lint's rung instead: the guard is
  `#[must_use]`, and `clippy::let_underscore_must_use` refuses the discard
  where it is on — the framework, the demo, and every workspace `nestrs new`
  writes, whose root denies it and whose members inherit it (correctness: the
  lint is in clippy's restriction group, off by default, so a scaffold that
  left it off would leave the trap open in the developer's own app, and a
  swallowed `Result` with it). A project the CLI did not write turns it on
  itself, which the OpenTelemetry page says.
- **A reload slot filled by `OpenTelemetryModule::for_root(config)`.** The
  `reload` layer takes a lock on every span and event, which every app would
  pay whether it exports or not, and what is emitted before the module
  registers would reach the console only.
- **Keeping `<PREFIX>_OPENTELEMETRY__*` beside `OTEL_*`.** Two names for one
  setting, one of them invented: 7.0 is the major, so no alias.

## 2026-10-30 — the exporter selectors and `OTEL_SDK_DISABLED` are honoured or refused, and the console's variables overlay a pinned config

Reading the specification's endpoints without its switches was a subset passed
off as support: a deployment setting `OTEL_TRACES_EXPORTER=none` beside
`OTEL_EXPORTER_OTLP_ENDPOINT` still shipped its traces. So:

- **`OTEL_TRACES_EXPORTER`, `OTEL_METRICS_EXPORTER` and `OTEL_LOGS_EXPORTER`
  are read here**, under the specification's names (no crate of the SDK names
  them): `otlp` — the default — or `none`, case-insensitive, or a
  comma-separated list of `otlp`, as the specification allows. `none` switches
  the signal off whatever endpoint names it, and its endpoint, protocol and
  certificate variables are then not read.
- **Anything else is refused at boot**, naming the variable and what it
  accepts, never the value: an exporter the specification knows that this
  crate does not build (`console`, `zipkin`, `prometheus`, `otlp/stdout`), a
  misspelling, a list naming nothing, and `none` beside `otlp`. The
  specification asks an implementation to warn and ignore an enum value it
  does not recognise; correctness outranks the standard's letter here, since a
  deployment believing a signal ships where nothing ships is the defect — the
  reference SDK's autoconfiguration (Java) refuses an unknown exporter name and
  `none` beside another at start too. The hard "no" on quoting a value
  outranks the specification's wish to display it.
- **`otlp` named for a signal no endpoint reaches is refused**, naming the
  endpoint variables. An explicit request is never dropped in silence
  (correctness), and the specification's fallback for it,
  `http://localhost:4318`, is a plaintext export no line wrote (security). An
  unset selector with no endpoint stays quiet, as before, and the boot line
  says `off (no endpoint)`.
- **A binary built without the `otlp` feature reads the selectors too**, and
  refuses any value but `none` — `otlp` included — naming the variable and the
  feature: it builds no exporter, and an explicit request is never dropped in
  silence. `OTEL_SDK_DISABLED` is not read there: such a binary has no SDK to
  disable, and no value of it asks for an export.
- **`OTEL_SDK_DISABLED` is honoured as the specification orders**: `true`, in
  any case, switches every signal off; anything else leaves the SDK on, and a
  value neither `true` nor `false` is said at `warn` — the specification's
  boolean rule ("MUST be interpreted as false"), whose false is the safe
  default. The selectors are still checked under it.
- **The boot line says where each signal goes**: its endpoint, or
  `off (OTEL_<SIGNAL>_EXPORTER=none)`, `off (OTEL_SDK_DISABLED=true)` or
  `off (no endpoint)`.
- **`init_with` lays the console's variables over the config it takes, field
  by field**, through the kernel's `LogSettings::overlaid_by_env`, which reads
  and checks each as `from_env` does, and checks the filter set in code
  whatever the variables set: a pinned base is the base the environment
  overlays, and a filter that does not parse is a defect of the code. Every variable this crate reads — `OTEL_*` and the
  console's — comes from the process environment, which `#[nest_rs::main]`
  fills with the `.env` cascade, so a pin here does not cut the cascade off as
  a `#[config]` pin does: telemetry starts before any module, from no
  `ConfigService`, and `init()` — whose base is `from_env` — must keep reading
  `.env`.

**Refused:**

- **Warning and ignoring an exporter this crate does not build**, as the
  specification's enum rule reads: above.
- **Building the specification's other exporters, gRPC or http/json**: a new
  exporter is a dependency decision, and the OTLP exporter specification lets
  an SDK build http/protobuf alone; they stay refused at boot.
- **Reading the console's variables over a pin from the deployment tier
  alone**, as a `#[config]` pin does: `OTEL_*` would then need the same cut,
  and `init()` would stop reading a `.env` file's `OTEL_*`.
