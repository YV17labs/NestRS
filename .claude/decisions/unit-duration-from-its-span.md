# A unit's duration is recorded from its span, keyed by the server's values

Settled on 2026-11-12, at the owner's request to give every app the metrics
its edges' conventions name — the requests, errors and latency a backend's
ready-made dashboards read — with nothing written, after comparing
`@nestjs/observe`, whose SDK times every unit and ships the result to a hosted
backend of its own.

A unit whose edge's conventions name a duration histogram declares it beside
the unit (`unit!`'s `duration:`, a `DurationHistogram`, which a client operation
names too): `http.request` records in
`http.server.request.duration`, `queue.job` in `messaging.process.duration`,
`mcp.operation` in `mcp.server.operation.duration`. `nest-rs-opentelemetry`
installs a layer timing each such span from its opening to the end its edge
records (`record_end`, which `record_outcome` calls), keyed by
the span fields the declaration names, under a filter of its own, installed
only when the metrics export.

Refused:

- **Timing to the span's close** (until 2026-11-13): a child span holds its
  parent open, and so does a `TaskContext` carrying work past the answer, so a
  request answered in 5 ms with 30 s of work handed off read 30 s. The span's
  close is the end only for a unit whose edge records none.

- **Deriving the metrics in the collector** (a spanmetrics connector): it
  counts the spans it was sent, so a sampler keeping one trace in a hundred
  divides every rate by a hundred, and a deployment without that collector has
  no metric at all.
- **Recording at the operation line**: HTTP files no line with its access log
  off, and `nest_rs::operation=off` silences every line, so a console setting
  would switch the metrics off.
- **One filter in front of every layer**: `RUST_LOG=warn` disabled the
  operation spans, and with them the durations and the exported traces. The log
  filter stands in front of the console and the log bridge together, the
  durations carry their own (`MeasuredUnits`), and the trace export takes the
  log filter or every unit's span where traces export (`UnitSpans`, added
  2026-11-13): the sampler decides which traces leave, `RUST_LOG` what the
  console writes. A filter per log layer was refused, since filters that
  disagree on a callsite leave it to be asked on every event.
- **Keying by whatever the span holds**: a span keeps what the conventions
  require as the client sent it (an MCP method only the client named), and a
  metric attribute's every value is a series a backend keeps. A field the
  server bounds is `MetricAttribute::bounded`; one the client chose is
  `MetricAttribute::known`, read through the set it may take and `_OTHER`
  otherwise.
- **A duration for GraphQL, WebSocket, scheduled and event units**: no
  convention names one, and a name the framework invented would be a lookup
  table no backend's dashboard reads. Their spans carry their timings.
