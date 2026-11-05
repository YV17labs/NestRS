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

## 2026-11-05 — A binding imports its resource, and an adapter the throttler its host binds

The same reasoning for the two lines a root still wrote for someone else
(the identity audit's second pass, comp-4, rules-5 and comp-6's reshaped half,
agreed with the owner's "Tout faire (Recommandé)", 2026-10-31). Elegance decides:
security and correctness are untouched, and each line chose nothing.

Adopted:

- **Each `<Vendor><Port>Module<I>` imports `<Vendor>Module<I>` bare, from both
  phases**: `SeaOrmDatabaseModule` the pool, `RedisQueueModule<I>`,
  `RedisScheduleModule<I>` and `RedisThrottlerModule<I>` their instance's
  connection. The root names the resource only to pin it, which supersedes the
  bare import in either order, and a seed still wins over a factory. What refused
  a binding left without its resource (`SUBSTRATE_REMEDY`, `connection_remedy`) is
  gone: nothing reaches it. A provider injecting a resource nothing opens keeps
  its `DependencyRemedy`.
- **An adapter whose host binds `ThrottlerGuard` imports `ThrottlerModule` bare**,
  the port behind that guard, so an app serving no such adapter pays no rate
  limiter. A root's `RedisThrottlerModule` still supersedes the in-process store
  wherever the two fall, so swapping the backend still edits the root alone. A host
  binding the guard, a root's `GlobalGuards` listing it or a module listing it by
  hand, with nothing importing the port, is told `ThrottlerModule` through the
  `DependencyRemedy` of what each misses. A root whose pool lists the guard imports
  the port itself: the pool is its own binding, which no adapter's import should
  hold up.

The binding itself stays the root's, as this page's adoption says of the queue's:
it is what the root chooses, and what it runs on is not.

Refused:

- **Changing how a guard and its strategy are listed** (the audit's comp-6, as
  first written, which read three ways for a guard to reach its host): listing a
  guard in `providers` is the general rule and `DevelopmentOnlyGuard` follows it,
  `ThrottlerGuard` is the one built by its port module because it reads the port's
  policy and store, and a guard registering its own strategy, its type parameter,
  would be a second registration path that collides when two guards share one.

## 2026-11-05 — A host brings what its edge runs on, and the adapter names its port

The refusal of *Activation from the decorator* gave two reasons: a phase limit,
and "the adapter's import is the visible line". Neither ranks above elegance. The
limit was the framework's own, and the edge decorator on a listed provider is as
visible as the import it replaced; the line that opens an edge is the root's
import of the adapter, which stays. Every adapter paid a line for what its
provider's decorator already named, two per gateway, and the import leaked what a
transport rides on (the identity audit's second pass, comp-5, agreed with the
owner's "Tout faire (Recommandé)", 2026-10-31). The refusal is reopened.

Adopted:

- **A decorator making a provider an edge's host submits what the edge runs on**,
  a `HostRuntime` naming the modules its edge's crate declares (`RUNTIME`):
  `HttpModule` for `#[routes]`, it and `WsModule` for `#[messages]`,
  `GraphqlModule` for `#[operations]`, `McpModule` for `#[mcp]`, `QueueModule` for
  `#[processor]`, `ScheduleModule` for `#[scheduled]`. `#[module]` imports them
  bare for each provider it lists, in both phases, after its `imports`, and the
  access graph walks them as imports, so a gateway reaches `WsServer` and a
  discovery reader reaches what they list. The two provider-hosted decorators
  submit per dispatched method under its `#[cfg]`s: a host whose methods are all
  compiled out runs nothing, so it must not attach a worker that refuses an app
  binding no backend.
- **An inventory entry, not a hook on `Discoverable`.** The audit proposed a
  defaulted hook, but `#[processor]` and `#[scheduled]` sit on an `impl` whose
  struct's `Discoverable` is `#[injectable]`'s, so they could not override it;
  they would have needed this entry beside it, two ways to one end. `HostLayers`
  and `HostRequires` already carry what a decorator declares of a host it did not
  build. An entry is read only by the `#[module]` listing its host, so module
  gating holds: what no module the app reaches lists brings nothing.
- **The backstop is gone for edges.** A host a reached module lists now always
  has its runtime, so `host not served: its edge's transport is not attached`
  could fire on nothing. `App::run` keeps the line for an indicator's host.
- **An adapter importing its transport stays valid**, an idempotent bare import.
  A module listing no host keeps it for good (`AuditHttpModule` lists an
  interceptor), as a port pushing to clients keeps `WsModule`.

Refused:

- **`HealthModule` from `#[indicators]`, `EventsModule` from `#[listeners]`.**
  Neither is an edge's host in an adapter: an indicator sits on a port or a
  resource's provider, and `HealthModule` mounts `/health` on HTTP, so bringing it
  would open a listener in any binary reaching that port, a worker included, with
  no adapter line naming it — the `WsModule` refusal above, and the reason an
  indicator without the probes keeps its line at run. The bus stays at the port
  that emits.

The `WsModule` refusal stands: `HttpModule` comes from a gateway, never from
`WsModule`, so a port importing the registry opens nothing. What this moves onto a
developer: a host listed by a port now serves wherever the port is imported, which
is why `features.md` keeps hosts in adapters; a module listing a host in a binary
that was not meant to serve its edge now listens or refuses instead of idling, and
a container built by hand seeds what a listed host's module reads, as it does for
an import written out. Both are in the upgrading guide.
