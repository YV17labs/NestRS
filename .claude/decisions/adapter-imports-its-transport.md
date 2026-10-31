# An edge adapter imports its transport, and a host left without one is named at run (7.0)

The identity audit of 2026-10-28 found seven edges and three ways to activate
one: a WebSocket adapter imported `WsModule` itself, an `#[mcp]` host mounted by
being listed, and HTTP, GraphQL, the queue and the schedule waited for the app
to import `HttpModule`, `GraphqlModule`, `QueueModule` or `ScheduleModule` a
second time beside the adapter. Left out, the adapter the app had just imported
did nothing and nothing said so: an `#[scheduled]` method compiled in and never
ticked, a worker idled, and the GraphQL quick start served nothing to the
`curl` that followed it, since no framework module mounting on HTTP imported
`HttpModule`. An app with no transport at all logged `app ready transports=""`,
then `app draining cause=TransportFailed`, and exited 0. The owner agreed to
the change with the audit's other fifteen (2026-10-28, "Tout faire").

Adopted:

- **Every `<Feature><Edge>Module` imports its transport module bare** —
  `HttpModule` for HTTP, WebSocket and MCP, plus `GraphqlModule`, `WsModule`,
  `QueueModule` or `ScheduleModule` — and every framework module mounting on
  HTTP (`GraphqlModule`, `HealthModule`, `OpenApiModule`, `McpModule`, the
  OAuth resource server's, `StaticFilesModule`) imports `HttpModule` bare. A
  bare import is idempotent and the app's pinned `for_root` supersedes it, so
  the root names a transport only to pin it. Events' bus stays at the port
  that emits, as `posts` does. `nestrs g <edge>` writes the import, and its
  "make sure the app imports …" step is gone.
- **The queue's backend binding stays at the root** (`RedisQueueModule`): the
  backend is the composition's choice (`architecture.md`, *swapping a backend
  edits the composition root alone*). `g queue` names it only when the app's
  root binds none.
- **The backstop is a line, not a refusal.** `App::run`, once its transports
  are attached, files one `warn` per host the app reaches whose edge's runtime
  is absent — a controller, a gateway, a resolver, an `#[mcp]`, `#[process]` or
  `#[scheduled]` host — naming the host, its module and the import, each
  edge's crate declaring what its hosts run on through core's `EdgeRuntime`
  registry. A run with no transport files its own `warn` and stops for
  `StopCause::NoTransport`.

Refused:

- **`WsModule` importing `HttpModule`**, which the audit's synthesis listed.
  `WsModule` mounts nothing — a `#[gateway]` self-mounts — and it is the
  registry a port imports to push to clients (`chat`), so importing HTTP there
  would open a listener in any binary reaching that port, a worker included,
  with no adapter line naming it. Each WebSocket adapter imports `HttpModule`
  and `WsModule` itself instead.
- **Activation from the decorator** (`#[scheduled]` importing `ScheduleModule`
  from its `Discoverable::register`): `register` runs after the collect phase,
  so a module queueing its config there is refused as a late factory. It would
  need a collect-phase hook on `Discoverable`; the adapter's import is the
  visible line instead.
- **A seal-time check, or a refusal.** `TestApp` and `HeadlessApp` serve hosts
  on transports they build, with no `HttpModule` or `ScheduleModule` imported,
  so a check at the seal would warn on, or refuse, working compositions; and
  turning a booting app into a refusal is a break. Escalating either line to a
  refusal is the owner's call.
- **The guard-declaring authz adapters importing a transport**
  (`AuthzGraphqlModule`, `AuthzMcpModule`): they serve no unit, and their
  `EndpointGuards` declaration already fails the boot when the app mounts no
  endpoint for it, which an import would hide.

The modular monolith's opening is unchanged
(`modular-monolith-per-workload.md`): what a binary does not import is not
mounted, and the adapter import is the line that opens an edge.

## 2026-10-31 — `g queue` writes the backend binding into the root that has none

The binding still sits at the root, where swapping the backend edits one
line; what moved is who writes it there. `g queue` now writes
`RedisQueueModule` into the app root it wires when that root binds no queue
backend, with `RedisModule` unless the root opens the default instance the
bare binding reads, in the same edit as the adapter, rather than printing them —
as `g resource` writes the data layer (`generator-wires-its-app.md`). A
printed line was a step the generator could take itself, which the
identity's "nothing to wire" refuses (the identity audit's read-12, agreed
with the owner's "Tout faire", 2026-10-28). A root binding a backend already
— bare, pinned or a named instance — keeps its own line.

## 2026-10-31 — An MCP adapter imports `McpModule`, so the MCP config is read

The adopted line put MCP under `HttpModule` with no reason given, and it cost
the config: `McpConfig` loads only where `McpModule` is imported, so an app
whose MCP adapter imported `HttpModule` and whose root imported nothing of
MCP's read no `<PREFIX>_MCP__*` variable — the Host allowlist included, which
the mount's own refusal tells the operator to set. `nestrs g mcp` and the docs
wrote that shape; the demo read its config only because its root pinned an
identity (the identity audit's second pass, comp-1, agreed with the owner's
"Tout faire (Recommandé)", 2026-10-31). Correctness decides.

Adopted: **an MCP adapter imports `McpModule` bare**, as a GraphQL adapter
imports `GraphqlModule`. `McpModule` reads `<PREFIX>_MCP__*` and imports
`HttpModule` bare, so the adapter names one module and the app importing it
serves the endpoint on the deployment's config. The root's
`McpModule::for_root(…)` stays only to declare the identity, the opening or a
pinned base, which supersedes the bare import (`McpSetup` imports the module by
type). The run-time backstop names `nest_rs::mcp::McpModule` for an `#[mcp]`
host left unserved; whether one is served is still read off the HTTP transport.

The `WsModule` refusal does not reach this: `McpModule` mounts nothing a port
imports to push with, and already imported `HttpModule`. A host listed beside
`HttpModule` alone still mounts on `McpConfig::default()`: what the harnesses
serve, and a hand-written adapter's old shape, until an edge decorator brings
its edge's module itself.
