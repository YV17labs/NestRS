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
