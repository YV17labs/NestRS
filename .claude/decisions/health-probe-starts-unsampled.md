# A health probe starts its trace unsampled, and a healthy answer files at `debug`

Settled on 2026-11-14, from the `@nestjs/observe` audit (its `http.ignore`), the
owner deciding the line's level.

Every probe opened an `http.request` span the default sampler exported and filed
its line at `info` on `nest_rs::operation`, which the logs bridge exports too:
three probes every 10 s are 18 traces and 18 lines a minute per pod, and nothing
in the app could drop them, the sampler being the specification's closed set
(`telemetry-reads-the-specification.md`). Seen by tests, red before the change:
`GET /health/live`'s span reached the collector (`trace_flags=03`), and its `200`
line was at `info`.

- **The crate mounting a route declares it a probe**: `nest-rs-health` attaches
  `ProbeHost` (`nest_rs_http::__private`) to its controller from its module's
  `register`, gated by construction, and the transport reads every route of that
  controller as a probe at its literal served path, the global prefix included.
  No setting, no public decorator.
- **The edge knows a probe before it opens the request's span**, by the method
  and the path as the router reads them (trailing slashes trimmed, `HEAD` served
  by a `GET`): the sampler decides as the span opens, before the router matches.
  An app with no probe pays one `is_empty` a request, one with the three probes
  three short comparisons.
- **A probe that starts a trace starts it unsampled** (`minted_unsampled`), and
  the sampler `init` installs, wrapped around the configured one
  (`AdoptUnsampledStarts`), keeps that verdict: nothing of the trace is exported,
  and its `traceresponse` says it unsampled. A probe a trusted proxy continued
  keeps the configured sampler's verdict, its parent's under the default
  `parentbased_*`.
- **Its duration still records** in `http.server.request.duration`, which the
  timing layer reads off the span whatever the sampler keeps: an availability
  objective is computed from it.
- **Its line files at `debug` while it answers `2xx`**, and at `info` otherwise —
  a `503`, a body that unwound, a cut — as every request's does: the one exception
  to `info` for an edge's success, written in `observability.md`. `operation_line!`
  spells it with its `debug:` key.
- **Family**: no other edge serves a probe. A probe is the HTTP `GET` an
  orchestrator sends; WebSocket, GraphQL, MCP, the queue, the schedule and the
  events have none.

Refused:

- **A public `#[untraced]`, or a configured list of paths**: no caller but the
  probes exists, and a list drifts from the routes. The seam is on the route
  table, so a decorator can reuse it the day a caller exists.
- **Dropping the probe's span in a span processor once it ended**: the trace was
  sampled meanwhile — its `traceresponse` and every outbound `traceparent` said
  so, and a child span was exported — and the processor would read every span to
  find it.
- **Taking the sampler's verdict once the router matched**: the ids the exporter
  adopts are pending only while the span is created, and every client operation
  under the unit reads the verdict.
- **Matching the router a second time ahead of the span**: a route lookup a
  request, on every request, for three literal paths.
