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

## 2026-10-31 — every variable for what the crate builds is honoured or refused

The entry above left variables a deployment sets passed over: a compression
the SDK warned about and dropped, a propagator list and a configuration file
nothing read, a metric reader's bound nobody applied, an endpoint a binary
without the exporter ignored. Each is a deployment believing what the binary
does not do. So:

- **`OTEL_EXPORTER_OTLP_COMPRESSION` (and per signal) is resolved here and
  handed to the exporter**: `gzip`, the one compression the OTLP
  specification names, under opentelemetry-otlp's `gzip-http` (flate2, already
  locked), or `none`, in any case. `zstd` is refused at boot: the SDK offers
  it, the specification does not, and its feature would build a second copy of
  the zstd C library beside the HTTP edge's (opentelemetry-otlp 0.33 asks zstd
  0.13 where the workspace locks 0.14). A value is matched untrimmed, as the
  SDK matches it, so the two never read one apart.
- **The timeout and the compression are handed to each exporter**, never left
  for the SDK to read again its own way: its retry deadline is then this
  crate's bound whatever the variable's spelling.
- **`OTEL_METRIC_EXPORT_TIMEOUT` bounds a metric export beside the exporter's
  own timeout, and the earlier cuts it.** The specification defines both, at
  30 s and 10 s; reading the reader's alone when the exporter's is unset, as
  first proposed, would let it raise the exporter's specified 10 s. Not a
  whole number of milliseconds is passed over and said, as its siblings are.
- **`OTEL_PROPAGATORS` may only name what the framework propagates**:
  `tracecontext` and `baggage`, its default, in any order and case. The kernel
  propagates W3C Trace Context at every edge, and the queue carries a job's
  W3C Baggage onto every push it makes — the one place baggage reaches; no
  HTTP, WebSocket, GraphQL or MCP request reads or answers it. Neither goes
  through the SDK's propagator, so a format the framework does not speak
  (`b3`, `jaeger`, `xray`), a list leaving one of the two out, and `none` —
  which cannot turn a correlation the framework owns off — are refused. A
  list naming `tracecontext` alone is refused too, though an app with no
  queue would honour it: the crate cannot see whether the queue is linked,
  and a deployment believing baggage is not carried where it is would be the
  defect. The SDK's global propagator, for what reads it outside the
  framework, holds both formats, the specification's default, where it held
  Trace Context alone.
- **The sampler and the metric temporality, which the SDK passes over at
  `warn` quoting the value, are resolved here and handed to the builders.**
  `OTEL_TRACES_SAMPLER` names a sampler the SDK builds, matched as the SDK
  matches it (lowercase), so the SDK never reads it apart; `jaeger_remote`,
  `xray` and a misspelling are refused, where the SDK would sample every trace
  over the pinned ratio. `OTEL_TRACES_SAMPLER_ARG` reads as a ratio between 0
  and 1 or is refused — the specification would ignore it, but ignoring it
  samples every trace — and one beside a sampler that takes none is passed
  over and said. `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE` is read
  by the SDK's own parser and refused when it does not parse.
- **`OTEL_CONFIG_FILE` (and its experimental name) is refused at boot**: the
  Rust SDK has no file configuration, so nothing in the file would apply.
- **A binary built without the `otlp` feature refuses an endpoint** —
  `OTEL_EXPORTER_OTLP_ENDPOINT`, a signal's own, or a pinned `otlp_endpoint` —
  for a signal its selector leaves on, naming it and the feature, and reads
  the propagators and the configuration file as every build does. It now
  reads `OTEL_SDK_DISABLED` too, where the entry above said it did not:
  `true` says nothing exports, which is what such a binary does, so the
  endpoint is no refusal there.
- **A header beside a plaintext endpoint on another host is refused** — the
  security decision this entry was asked for. `OTEL_EXPORTER_OTLP_HEADERS` (or
  a signal's own), which the SDK sends with every export, is where a
  collector's credential travels; over `http://` it crosses the network in
  clear, where RFC 6750 §5.3 sends a bearer token over TLS alone (and RFC
  7617 §4 a Basic one). No header's name tells a credential from another (vendors
  name theirs `x-api-key`, `dd-api-key`, `x-honeycomb-team`), so every header
  is held to it. The endpoint's own query and userinfo are held to it too:
  every request line carries the query, a proxy's request line the userinfo
  (reqwest 0.13 sends `http://user:pass@host/…` to an `HTTP_PROXY` whole), and
  no parameter's name tells a credential apart either; the variable or field
  that set the endpoint is named. On this host's loopback — `localhost`,
  `127.0.0.0/8`, `::1`, in every profile — the credential never leaves the
  machine, and a collector beside the app is the common plaintext shape, so it
  is sent there.
- **A loopback collector is dialled directly**, past any proxy the environment
  names (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`): reqwest takes the
  environment's proxy for every host, loopback included, unless `NO_PROXY`
  lists it, so the exemption above would otherwise hand the credential, in
  clear, to the proxy — which reaches its own host's loopback, the wrong
  collector whatever the scheme. Any other endpoint keeps the environment's
  proxy, as an egress proxy expects.
- **The boot line's per-signal state is a string**, which the console quotes
  (`traces="off (no endpoint)"`), so a logfmt reader takes it whole; so is
  `service`.

**Refused:**

- **A `warn` naming the header variable, and the export sent** (the
  deployment wrote `http://`, an explicit opening): the credential would still
  ship in clear at every export, and safe by default outranks a line nobody
  acts on. The opening `http://` declares is the telemetry's, not a
  credential's.
- **Refusing every header over plaintext, loopback included**: it refuses the
  one plaintext shape where the credential never leaves the host.
- **Refusing `Authorization` alone**: a vendor's key header is a credential no
  standard names, and telling them apart by name is invented.
- **A service mesh's encryption as an exemption**: the crate cannot see a
  mesh, and a header that relies on one ships in clear wherever it is absent
  or bypassed; such a deployment writes `https://`.
- **Refusing a loopback endpoint while a proxy is set**, rather than dialling
  it directly: a proxy cannot reach this host's loopback, so there is no
  deployment the refusal would serve.
- **Honouring `zstd`**, above.

## 2026-10-31 — no variable, endpoint or header is passed over in silence

The entry above left a deployment still able to believe what the binary does
not do: a number the SDK could not parse replaced by its default with no word,
specification variables nothing read, a signal's empty headers variable read
apart from the specification, a credential in an endpoint's userinfo dropped by
the client, and a query on the base endpoint swallowing the signal's path. So:

- **A number the SDK reads that does not parse is refused at boot**, naming it
  and what it accepts: the batch processors' `OTEL_BSP_SCHEDULE_DELAY`,
  `_MAX_QUEUE_SIZE`, `_MAX_EXPORT_BATCH_SIZE` and their `OTEL_BLRP_*` twins,
  matched as opentelemetry_sdk 0.33 matches them (`from_str` on the raw value,
  so ` 512` too), and, for one rule over one class, the export timeouts, the
  processors' export bounds, the span limits and `OTEL_METRIC_EXPORT_INTERVAL`.
  The specification would warn and ignore an unparsable number; the reference
  SDK's autoconfiguration (Java) refuses one at start, and a deployment
  believing its value applies is the defect, as the 2026-10-30 entry decided
  for an exporter name. The refusal says the bound of the type the number is
  read as, since a number past it is a whole number still. The entry above's
  "not a whole number of milliseconds is passed over and said" no longer
  holds.
- **What the SDK has a setting for but reads nowhere is read here and handed
  over**: `OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT` and `OTEL_LINK_ATTRIBUTE_COUNT_LIMIT`
  with the span limits the SDK reads, each attribute count falling back on
  `OTEL_ATTRIBUTE_COUNT_LIMIT`, as the specification orders a model's limit
  before the general one (its attribute-limits section names a span's; this
  reads an event's and a link's as models of their own); and
  `OTEL_BSP_EXPORT_TIMEOUT` and `OTEL_BLRP_EXPORT_TIMEOUT`, which the
  thread-based processors this crate builds ignore, as the earlier bound beside
  the exporter's own, as `OTEL_METRIC_EXPORT_TIMEOUT` already is.
- **A specification variable no linked crate reads nor offers a setting for is
  said once at `warn`, never refused**: `OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT`,
  `OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT`, `OTEL_LOGRECORD_ATTRIBUTE_*`,
  `OTEL_METRICS_EXEMPLAR_FILTER`,
  `OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION`, `OTEL_LOG_LEVEL`,
  the SDK's own `OTEL_BSP_MAX_CONCURRENT_EXPORTS`, and `OTEL_ATTRIBUTE_COUNT_LIMIT`
  while the logs export (no log record's attributes are limited). Decided by
  the values: what is lost is a shape of data that still ships whole, not the
  data, and each is one a platform sets for every service's SDK — refusing it
  would stop an app over a variable its SDK does not implement, which a refusal
  of an exporter name, where nothing ships, does not weigh against. A value that
  names what the build does (`always_off`, `explicit_bucket_histogram`, `1`) is
  honoured in silence: a line on correct configuration teaches operators to
  filter the target. The line is the passed-over one, now `an OpenTelemetry
  variable is passed over` with `variable` and `reason`.
- **The headers are read here and handed to each exporter.** The
  specification reads an empty variable as unset; opentelemetry-otlp 0.33 reads
  a signal's empty headers variable as no header for that signal, over the
  general one. Handed over, the general headers reach that signal, as the
  specification orders, and the plaintext refusal above, already the
  specification's reading, is what the SDK sends. A pair the SDK would drop in
  silence — no `=`, an empty key or value, an invalid name or value — is
  refused; a value is checked decoded as the SDK decodes it, an escape that
  does not decode sent as written, so the two never read a pair apart, and
  is handed over as written, since the exporter decodes what it is handed:
  decoded once, never twice.
- **`OTEL_RESOURCE_ATTRIBUTES` is read here and laid over the SDK's
  detectors**, so the 2026-10-30 entry's "nothing overrides" no longer holds
  for it: opentelemetry_sdk 0.33's detector drops a member with no `=`, keeps
  one with an empty key, and decodes nothing, while the resource
  specification has the `,` and `=` of a key or value percent-encoded, other
  characters optionally (RFC 3986 §2.1), and discards the whole variable on a
  failure to decode. Each key and value is decoded; a member the SDK would
  drop or keep under no name, and an escape that does not decode, refuse the
  boot naming the variable — the reference SDK's autoconfiguration refuses a
  member with no `=` at start too.
- **Userinfo in any endpoint is refused**, the variables and the pinned field,
  over `https://` and to loopback too, naming the setter and pointing to
  `OTEL_EXPORTER_OTLP_HEADERS`, never quoting it: reqwest builds the request
  without it, so the credential reached no collector, which answered 401.
- **A query on the base endpoint is refused**, the variable and the pinned
  field. The specification sends each signal to a path relative to the base
  and lets an SDK change the URL no other way: a relative path drops the base's
  query (RFC 3986 §5.2.2), and the SDK's string append hid the path inside it.
  The signal's path is now set on the parsed URL, so a fragment, which no
  request sends, stays one; a signal's own endpoint is used as given, query
  and all.
- **The kernel's text console is coloured on a terminal alone** (the
  observability rule's formatters): tracing-subscriber's default coloured any
  stream unless `NO_COLOR` was set, so a logfmt reader of a piped console met
  escapes around every key. The console now turns colour off where stdout is
  not a terminal, and keeps the library's `NO_COLOR` reading (no-color.org) on
  one; JSON never carried any.

**Refused:**

- **Refusing the variables no linked crate serves** (`OTEL_LOG_LEVEL` first):
  above.
- **Saying nothing of them**: a deployment would keep believing what the
  binary does not do.
- **Mirroring the SDK's reading of an empty headers variable** (no header for
  that signal): a credential the deployment set for every signal would be
  dropped for one, the 401 the userinfo refusal ends. Refusing the empty
  variable as ambiguous was weighed and refused too: the specification gives
  it one meaning.
- **Keeping a base endpoint's query beside the path**: a reading neither the
  specification nor any SDK gives, so a deployment could not tell what its
  query reaches.
- **Reading `NO_COLOR` in the kernel**: the library reads it as no-color.org
  writes it, and a second reading would be a second implementation.
