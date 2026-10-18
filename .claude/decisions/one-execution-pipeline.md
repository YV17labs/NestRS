# One pipeline in core runs every unit, and the access decision comes first

Interceptors, filters and exception filters are three crates typed over poem
(`Interceptor::intercept(poem::Request, Next) -> poem::Result<Response>`), and
the other edges refuse their attributes by name as "not bridged … yet". No
package can wrap a GraphQL field, a socket message, an MCP tool or a job, where
NestJS's interceptors and exception filters apply on every transport and its
official packages ship as one global interceptor reading handler metadata.
`#[interceptor]` builds its struct from a container snapshot, off the global
pool, so a type also listed globally runs twice, and route-scoped interceptors
run before `#[authorize]`'s class gate.

**One unit, one function.** A routed HTTP request, an HTTP self-mount's request,
the HTTP fallback, a GraphQL root field, a socket message, an MCP operation, a
queue attempt, a scheduled tick and an event listener each run through one
`dispatch` in `nest-rs-core`. Outermost first:

1. **The edge boundary**, the edge's own code: correlation, span, operation
   line, request scope, deadline, panic containment.
2. **The data unit of work**, which is not a layer: each edge calls the database
   port around `dispatch`, so guards run inside its executor scope and a denied
   mutation opens no transaction.
3. **The unit stage**: `#[interceptor(stage = unit)]`, brought by a module
   import (`Server-Timing`, the RFC 9728 challenge) — where NestJS middleware
   maps.
4. **The guard stage**: global, host, then method guards, then the class gate
   `#[authorize]` declares. What a guard attaches becomes ambient only once the
   whole stage passed; a denial is answered here and reaches no exception
   filter.
5. **Exception filters**: method, host, then global; the first that answers
   wins, and the unit stays failed.
6. **The handler stage**: the app-listed global interceptors, then the
   `#[interceptor]`s modules bring, in import order, then host, then method;
   first listed outermost.
7. **The tail**: pipes, the handler, the posture's mask, the reply written into
   the edge's view.

**What the one function holds on every edge:**

- Nothing an app or a package adds runs before the access decision, except a
  unit-stage interceptor, which says `stage = unit` in its source — an
  opening, so a visible line.
- A unit runs once: `Next::run(self, ..)` consumes the rest of it.
- A failure is sticky: the `Ok` a layer returns after its inner unit failed is
  turned back into an answered failure, the original error kept and one `warn`
  naming the layer, so the data unit still rolls back. An outer layer changes
  what the client reads, never whether the unit failed.
- A layer runs once per unit, deduplicated by type, unless `#[force_guards]`
  re-runs one — a visible line — and `Layer::priority` orders within one site's
  bucket, never across.
- Every edge renders a failure nobody answered by one rule: a deliberate client
  error in its structured form, anything else opaquely with its chain logged
  once at `error`.
- A nested unit (a root field inside its `/graphql` request) composes no unit
  stage and no global handler bucket.
- Chains are composed once per site and container, owned by the container,
  never by a `static`; a site with nothing bound builds no context at all.
  `Settled`, which only running or answering mints, makes a layer that does
  neither a compile error.

**The families live in `nest-rs-core`, not `nest-rs-http`**: the stage order is
written and tested once, a headless edge gets layers with no feature, and
NestJS keeps both families platform-neutral over its `ExecutionContext`. One
error-mapping family remains: `Filter` folds into `ExceptionFilter`, whose
catch-all is `type Exception = dyn Error + Send + Sync`, matched anywhere in the
failure's source chain. One `Reflector`, in core, reads what a handler
declared; a guard reads the request through the edge's view, the whole request
included.

**Refused:**

- **Global interceptors before the guards**: a package's cache would answer
  before the access decision.
- **The class gate after the handler-stage interceptors**: the same opening,
  scoped to a route.
- **Two error families** (`Filter` beside `ExceptionFilter`): two decorators
  for one concern.
- **A replayable `Next` in 7.0**: "a unit runs once" would become a convention
  instead of a type; a layer that retries is additive later.
- **Views borrowing an edge's internals through `unsafe`**: each view owns what
  it exposes.
- **Single-slot around seams** (one `SocketContext`, one `McpToolContext`, one
  `GraphqlBatchContext` per edge): a second concern cannot join a slot; it
  joins a chain.
- **The data context as a layer** (an interceptor in a transport band): a
  driver would publish a band instead of implementing one port.
- **A guard handed the request head apart from the request**: a guard verifying
  a signature must read the body.
- **An HTTP band API beside the unit stage**: two ways to be transport-wide.
- **Moving the HTTP-typed families into `nest-rs-http` first**: code moved
  twice, and a public path that would live for days.
- **The container on the context**: unchecked dynamic resolution stays out of
  7.0.
- **Layer bundles (`#[use_layers]`)**: additive, later.

**Status (2026-10-10): decided, not landed.** `feat/pipeline-core` adds the
contract beside today's families; each edge moves in its own branch
(`feat/http-on-pipeline`, `feat/graphql-on-pipeline`,
`feat/websocket-on-pipeline`, `feat/mcp-on-pipeline`,
`feat/jobs-ticks-listeners-on-pipeline`); `refactor/retire-replaced-contracts-and-poem`
removes the three crates. Each appends an entry here where it departs.

**2026-10-11 — the contract landed beside today's families, with five
departures.** `nest-rs-core` holds `Outcome`, `Settled`, `Failure`,
`Interceptor` and `Next`, `ExceptionFilter`, `Caught` and `Catch`, their specs,
`Layer::serves`, `#[interceptor]` and `#[interceptor(stage = unit)]` (a
singleton provider plus a registration its module attaches), and, tier 2, the
`Site` a decorator declares in a `static`, the `UnitChains` each container
composes for it once and holds, and `dispatch`. No edge dispatches through it
yet; the app's two lists are reached through `__private` until
`feat/http-on-pipeline` makes them `AppBuilder` methods. Where it departs:

- **`dispatch` is generic over the edge's tail**, whose `call` returns a future
  rather than a box: a `Bare` site runs its tail inline and allocates nothing,
  a `GateOnly` one boxes its guard stage alone, and only a `Layered` one boxes
  the tail, since `Next` must hold it erased. What is instantiated per tail is
  the shape switch; the stages are written once, behind the box. The switch
  is a future of its own, never an `async fn`, as the ambient installs are: a
  debug build would size its one resume frame for every arm's copy of the
  tail and keep it on the stack while the tail runs.
- **What a site folds of the app-wide layers is three switches**
  (`GlobalLayers`: the unit stage, the app's exception filters, the handler
  stage's app-wide interceptors), not one "nested" flag: a GraphQL root field
  folds the filters and the interceptors but not the unit stage its request
  ran, a self-mount the unit stage and the filters but no interceptor, MCP's
  other requests the unit stage alone, a field resolver none.
- **A filter's problem rides on the failure.** `Caught::Problem` leaves the
  unit failed and its error whole; the edge reads the problem off the
  failure and renders it over whatever its view held, filing a `5xx`'s
  withheld chain as for any other.
- **The handler stage's app-wide bucket is one site**: the app's list, then
  the modules' in import order, ordered by `Layer::priority` with that order
  breaking a tie — a priority orders the app's and the modules' together,
  never ahead of the host's or the method's. `serves` filters every app-wide
  list, the exception filters' too.
- **`force` is the guard stage's.** Guards are the one family with a
  `#[force_*]`, so the site's guard-stage factory carries it; no other list of
  the declaration takes one.

**2026-10-17 — `feat/endpoint-guards-and-class-gate`.** The posture's class
gate is `nest_rs_authz::class_gate::<A, S>`, which coerces to `ClassGate`;
every decorator emits it, so a package's decorator emits the same for its own
posture. One departure: a self-mount's request stage is composed by one
function in `nest-rs-http` (`request_stage`) rather than handed to a site as a
`GuardStageFactory`, since no self-mount declares a site before
`feat/http-on-pipeline`; that branch builds the factory over the same
function rather than composing the stage again.

**2026-10-18 — `feat/jobs-ticks-listeners-on-pipeline`.** A queue attempt, a
scheduled tick and an event listener call run through `dispatch`, with no
guard stage (`Posture::System`) and every app-wide layer, inside the unit of
work `run_unit_bounded` opens — the one place the body is cut at the
decorator's `timeout` and a commit unanswered its settle net past it is in
doubt, which the three edges had each written by hand. The worker and the
scheduler compose their sites at configure, the bus at its wiring step, so a
layer nothing provides fails the boot. Where it departs:

- **A provider-hosted site reports its layers through an inventory entry**
  (`HostLayers`), read by the access graph beside each provider's injects:
  the host's struct is `#[injectable]`'s, so no `Discoverable` the impl half
  writes could carry them, and an impl half cannot know its siblings to
  merge one per host.
- **A worker view answers by settling the unit, never by a reply**:
  `QueueView::acknowledge`, `ScheduleView::skip`, `EventsView::skip`. A job's
  disposition is the queue's rendering of a failure, so a filter's problem
  decides it (a `4xx` other than `408` and `429` dead-letters) and a failure
  answered by `acknowledge` settles the job done, its writes rolled back. A
  tick has no retry and a listener no dead letter, so a failure a layer
  answered there — `skip`, or a filter's client problem (a `4xx`) — is one
  the app expected, said at `warn` rather than `error`; a `5xx` problem stays
  the server's failure, at `error`, so an app-wide filter mapping every error
  to a problem never silences one.
- **A job attached by hand as a `CronJobMeta` runs under the scheduler's own
  handler**, which declares nothing: no decorator declared its site, so it
  binds no layer of its own, and the app's run around it.

**2026-10-18 — `feat/http-on-pipeline`: HTTP runs on it.** A route, a
self-mount's request and what the router answers itself are each one `Site`,
dispatched inside `run_unit` over the binding the transport resolves at mount;
`DbContext`, `MappedError`, the transport bands, the route shaper and the HTTP
`#[interceptor]` are gone, and `use_interceptors_global` /
`use_exception_filters_global` are `AppBuilder`'s own. Where it departs:

- **The tail answers a failure, and the edge renders one inside the unit
  stage.** A handler's `Err`, or a `4xx`/`5xx` it answered, is an answered
  failure the exception filters still see; a failure no filter answered is
  rendered by the edge's one rule through `Tail::answer_failure` before the
  unit stage returns, so a unit-stage interceptor reads the reply that goes
  out, a `401` included.
- **A self-mount's guard stage is the transport's request stage, handed to
  `compose_site`** rather than declared as a `GuardStageFactory`: a
  self-mount declares no site of its own, and the origin check stays in front
  of the site, so a refused origin pays for no guard.
- **A guard-stage factory refuses with `anyhow::Error`**, so a route whose
  guards are out of phase order refuses through the same return as a layer
  that does not resolve.
- **The fallback is a site with no guard stage and no unit of work**: a `404`
  through a catch-all at the router's root, a `405` through the route that
  matched its path; the app's exception filters and unit stage see both.
- **A request carrying units runs on the pool**, its body uncut, whatever its
  method: `/graphql` and `/mcp` operations settle their own work.
- **A self-mount folds no handler stage** until its edge's own units move
  (GraphQL, MCP): the app's global interceptors reach a route's handler,
  never a self-mount's request.
