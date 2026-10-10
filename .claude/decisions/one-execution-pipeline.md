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
