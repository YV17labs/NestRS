---
paths:
  - "crates/nest-rs-core/src/logging.rs"
  - "crates/nest-rs-core/src/trace_context.rs"
  - "crates/nest-rs-core/src/operation_log.rs"
  - "crates/nest-rs-core/src/panic.rs"
  - "crates/nest-rs-core/src/error_message.rs"
  - "crates/nest-rs-core/src/request_scope.rs"
  - "crates/nest-rs-http/src/trace_context.rs"
  - "crates/nest-rs-http/src/access_log.rs"
  - "crates/nest-rs-http/src/response_body.rs"
  - "crates/nest-rs-queue/src/envelope.rs"
  - "crates/nest-rs-opentelemetry/**"
  - "crates/*/src/unit.rs"
  - "crates/*/src/target.rs"
---

# Observability — trace context, operation spans and lines

`CLAUDE.md`, *Observability*, holds what every event obeys — its target, its
level per layer, a constant message plus fields, the error chain. This file is
the model behind it, and the target table.

## Correlation is W3C Trace Context, in the kernel

**The correlation primitive is W3C Trace Context and it lives in
`nest-rs-core`.** A `trace_id` names a whole distributed operation; a `span_id`
names one unit of work in it — an HTTP request, a WS message, an MCP operation,
a queue job, a scheduled tick — and each names the span that caused it. It is
minted by whoever accepts the work, or continued from what carried it. A
primitive cannot be optional, so it depends on no crate an app may decline, no
collector and no authentication. It is read through `current_trace_id()` and
`current_span_id()` at every edge; `current_traceparent()` is what an app injects
into its own outbound client. There is no separate request id:
`X-Request-Id` is upstream data, recorded as a captured header behind a trusted
peer, never an identity (`.claude/decisions/trace-context-over-request-id.md`).

- **Trust is one decision, one list.** An inbound `traceparent` is continued
  only from a peer in `trusted_proxies` — the evidence `X-Forwarded-For` is
  weighed on — and restarted otherwise, which is what the specification defines
  a front gate to do; an ungated one lets a public client pick its trace and set
  `sampled` on traffic it generates. `tracestate` is forwarded **verbatim**
  wherever the trace is continued, and dropped with its `traceparent` wherever
  it is not.
- **What the caller reads back is `traceresponse`**, the working group's
  response form — standards-track rather than a Recommendation, and stated as
  such. It carries the trace this service actually used, which at a front gate
  is deliberately not the one the caller asked for.
- **`actor_id` is an audit identity, never an authorization input.** The authn
  guard writes it when it resolves a principal; absent means anonymous. It is
  ambient (`set_actor_id`, write-once; `current_actor_id()` wherever the
  framework carries work) as well as on the span, and no sentinel is ever
  returned — `""` or `"anonymous"` would read as an actor of that name. What a
  caller may do is the ambient `Ability`; branching on `actor_id` in a service is
  an authorization decision outside a guard.
- **The sampling flag is shared and updatable**: a sampler decides after the
  span exists, and an outbound `traceparent` must not carry a decision nobody
  made.
- **The observability stack enriches; it never owns.** `nest-rs-opentelemetry`
  mints no identifier — its `IdGenerator` adopts the kernel's, so the export and
  the log lines name one request by one value. It adds the remote parent link
  and the sampler's verdict, and owns neither the span, the operation line nor
  the trace context.

## Propagation — task, process and response

The edge owes propagation at every boundary it crosses.

- **A task.** A framework `spawn` that does not carry the span roots its events
  at nothing; a new spawn is presumed to owe it until checked.
- **A process.** A queue job carries `traceparent` and `tracestate` in its
  envelope, and the consumer continues from them, so the job is a **child of the
  enqueue**. An envelope carrying none is a legacy or foreign payload and starts
  a trace, never a refusal.
- **A response.** **A unit of work ends when its answer ends, not when its
  handler returns.** A streaming body — `#[sse]`, a download, anything over a
  `Stream` — is polled after the handler, with the edge's task-locals unwound,
  so the HTTP edge re-installs the request (`RequestContinuation`) around every
  body poll, at the body, for every streaming response — **whatever the access
  log is set to**: a primitive true for short responses and false for long ones
  is absent exactly where a long operation needs it.
- **A continuation that outlives its request inherits identity, never
  resources.** A socket opened by an upgrade takes the upgrade's trace and actor
  and nothing else; pinning the request's scope for the socket's life is the
  defect. Where the framework dispatches inside the connection, the operation is
  the unit and the connection a field (`ws.message` carries `ws.connection_id`);
  where it cannot see one, the connection is the unit (`graphql.subscription`).
  Lifecycle hooks are units too (`ws.connect`, `ws.disconnect`): a hook is
  developer code that logs and writes like any handler.

## The operation span

**`operation_span!` is the only place the canonical fields are declared** —
`trace_id`, `span_id`, `parent_span_id`, `actor_id`. `tracing` fixes a span's
fields at creation, so recording a field no span declared is a silent no-op,
and declaring a field nothing fills is the same defect with the opposite sign.

- **The span reports the route template, never the path as addressed.**
  `http.route` is what a backend groups latency on, so an id in it is one group
  per id; the addressed path is `url.path`. The exported name is `otel.name`,
  `{method} {route}` once the router has matched, and the method alone when
  nothing matched — naming an unmatched span after its URL lets one scanner fill
  a tracing backend.
- **`otel.kind` values are constants in `operation_log::kind`**, and only the
  kinds this framework emits are declared; the edge that first needs another
  adds it.
- **The span says what the line says.** Wherever an edge files a line it records
  the outcome on the unit's span, so anything but `ok` exports as `error.type`
  with the same word and an `Error` status (an HTTP `5xx` records its status code
  instead, as the HTTP conventions ask). A request cut before it answered is
  named for the route its router matched (`nest_rs_http::matched`), never
  exported anonymous.

## What a log line carries

**Every log line carries the trace context of the unit that emitted it** —
`trace_id`, `span_id`, `actor_id` — **and no span state**: no span's attributes,
no span's name. The formatters (`nest_rs_core::logging::{TextFormat,
JsonFormat}`) are the same two whichever subscriber is mounted, and read the
ambient correlation rather than the span scope: a span's fields belong to the
span, `tracing` would render a whole scope (one `trace_id` per nested level), and
the context outlives the span where a streaming body runs. **Never write those
three as event fields.** **One event, said once**: a line restates nothing a
field or the enclosing span carries, and no two layers emit the same event.
**The formatter escapes every value**, so no field can forge a line.

Three deviations from OpenTelemetry's log data model, each deliberate:

- **`actor_id` is a top-level key.** The model's nearest equivalent is the
  `user.id` attribute; ours is an audit identity with framework-wide vocabulary,
  kept where an operator greps it.
- **`trace_flags` is JSON-only.** A JSON record may be joined against an export
  where the sampling bit decides whether the other half exists; a human at a
  console never acts on it.
- **`parent_span_id` is a span field and never a line field.** The model has no
  ancestry, and nothing renders a scope.

## The operation line

**Every edge files one line per unit of work, on `nest_rs::operation`.** The ids
relate lines; the line names the work — which route, event, job or tool — with
the edge's identity fields plus `outcome` and `duration_ms`, in **flat** field
names (a dotted name is ambiguous to `tracing` beside a path target). An edge
without it leaves its work anonymous on the console. No edge grows a config
toggle for it — `nest_rs::operation=off` is the family's one switch, and HTTP's
`access_log` pin is the one predating it — and the target names a category of
line rather than a subsystem (`.claude/decisions/operation-target.md`).

**A unit has one name, `<edge>.<unit>`, typed.** The crate that owns the edge
declares it with `unit!` in its `src/unit.rs` — the kernel holds none, since it
does not know which edges exist — from the closed edge vocabulary. The span
(`operation_span!`), the line's `name:` (exported as `event.name` by an OTLP log
bridge) and its message all read that one constant through `operation_line!`,
which files the line and records the outcome; a literal does not compile.

**A unit that does not settle still files its line**: `outcome = cancelled` for
one stopped first — by its caller, by the server at the shutdown signal, by the
shutdown window, with its worker — and `outcome = panic` for one that unwound.
No handler's return carries either, so the line is held by a guard dropped with
the unit's future (code that must notice a stop can miss it), and a panic is
contained where the unit is dispatched, unless the edge's transport takes the
connection down with it, as HTTP's does. Where a client waits, the unwind is
answered: an MCP internal error, a WS error frame with the socket kept, `1011` on
a socket whose connect hook or subscription unwound.

- **`panic` names the unit that unwound, never one torn down by it.** A guard may
  read an unwind in progress (`std::thread::panicking()`) only where nothing but
  its own unit shares the future it is dropped with. Where siblings share one —
  a GraphQL selection's fields — the unit catches its own unwind, files `panic`
  and resumes it, and a sibling dropped by it files `cancelled`.
- **The panic field is written through `nest_rs_core::panic`'s helper**, keyed
  by `panic::FIELD`, never a literal, and tests assert the constant.
- **Each edge's suite proves which of its units file `cancelled` and `panic`.**

## Targets are constants their concern's crate owns

A target is dotted, lowercase, and rooted at the crate that emits it:

| Emitting crate | Target |
|---|---|
| a `nest-rs-*` framework crate | `nest_rs::<concern>` — `nest_rs::http`; a family member roots at its family, `nest_rs::oauth::client` |
| the shared feature library | `features::<feature>` — each emitting feature declares `pub const TARGET` at its root (`features.md`) |
| an app crate, or a single-crate project | `<app>::<concern>` |

**Every string the framework interprets is a constant.** A literal target is a
typo away from an event no filter selects. **The crate that owns the concern
declares it** (`nest_rs_events::TARGET`, `nest_rs_seaorm::TARGET`): a central
table in the kernel would have `nest-rs-core` naming concerns it does not know
exist. **Owns, not emits** — a concern several crates emit on is declared once
by the crate the others already depend on and read from there. A crate owning
one concern spells it `TARGET` at its root; a crate owning several has a
`target` module (`nest_rs_core::target`, `nest_rs_http::target`). A `*-macros`
crate reaches its own surface crate's re-export (`macros.md`).

**The root is the crate, never the product**: a feature in `features` emits on
`features::…` whatever the binary is called, because a target's one job is to
say where an event was emitted. **A family member roots at its family**
(`nest_rs::oauth::client`), so `nest_rs::oauth=off` silences three role crates of
one standard — prefixing on purpose, a level a reader sees in the path. **No
other target is a raw-string prefix of another**: `EnvFilter` matches with
`starts_with`, so a toggle for one would silence the other
(`.claude/decisions/operation-target.md`). Held by the `filters` check in
`nest-rs-conformance`, over the declared target constants.
