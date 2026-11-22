# A call to a dependency runs under a named policy, and only the caller declares it replayable

## 2026-11-20 — `nest-rs-resilience`, ROADMAP § 5 item 1

An outbound call to a dependency the app does not control had nothing around it:
a slow partner held every request that called it, a down one was hammered by
every retry. A policy now bounds each attempt, attempts an idempotent call
again, opens a breaker on repeated failures and caps the calls in flight.

- **A policy is named by an instance marker**, one per dependency:
  `ResiliencePolicy<I>`, built by `ResilienceModule::<I>` from
  `<PREFIX>_RESILIENCE__<INSTANCE>__*` (`named-instances.md`, never a string
  key). Imported by type, one instance is one policy, so one breaker counts the
  failures of every caller in the process.
- **Replay is the caller's declaration**: `run` sends once, `run_idempotent`
  again after a failure or a timeout. The reqwest middleware reads it off the
  request: an idempotent method — RFC 9110 §9.2.2's, and `QUERY`, as `http`'s
  `Method::is_idempotent` reads them — or an `Idempotency-Key`
  (`draft-ietf-httpapi-idempotency-key-header`), with a body that can be sent
  twice.
- **Every `Err` is the dependency failing**; an answer it gave — a declined
  card — is the call's `Ok`. So the policy needs no classifier of its own, and
  the retry and the breaker count the same thing. The middleware alone reads
  statuses: a transport failure, a timeout, `408`, `429`, `500`, `502`, `503`,
  `504`.
- **A refusal is one class, `503` `UNAVAILABLE`**: the code module's own
  sentence for something the server depends on not answering, as a statement
  Postgres cancels already is. An open breaker carries the `Retry-After` it
  knows. The problem rides the error's chain, so the queue's retry honours it.
- **The breaker counts a window of the latest attempts**, the count window
  `@nestjs/resilience` and resilience4j count, and lets one probe through once
  its open time passed; a probe given up hands its turn to the next call. Each
  transition is a `warn` and one on `nestrs.resilience.breaker.transitions`.
- **The bulkhead's slot is held for the whole run**, its retries included: it
  bounds the callers a slow dependency holds, which is what it is for.
- **A wait the dependency asks** — a `Retry-After` in seconds — is waited whole,
  stretched by a fifth at most (`asked-wait-is-honoured.md`), up to the backoff's
  ceiling; past it the run ends with that answer, since a request cannot wait an
  hour.
- **A panic in the call counts as a failure and unwinds on**, for the unit to
  contain and file as it would without the policy.
- **The middleware sits in the port's crate**, behind its `reqwest` feature, as
  a guard implementing an edge's view sits behind that edge's feature: in
  `nest-rs-reqwest` it would reach the run, its verdict and its refusals as
  tier 2, a wider surface for one vendor.

**Refused:**

- **A method attribute beside the policy API** — the roadmap's open question.
  It would be a second way to do one thing, and it cannot say what only the call
  site knows: whether this call is idempotent. `macros.md`'s bar (three places,
  mechanical, one sentence) is not met by one call wrapped in one closure.
- **A fallback API.** The `Result` a run answers already is one:
  `.or_else(|_| cached())`.
- **`TIMEOUT` (`504`), `CIRCUIT_OPEN` and `BULKHEAD_FULL`.** A code is a class
  read off a standard (`every-refusal-carries-its-class.md`); the three are one
  class, and a `504` is a gateway's — the HTTP edge's own timeout answers `503`
  for that reason. The variant of `ResilienceError` tells them apart in code.
- **A breaker shared across replicas.** No store: a replica opens on what it saw,
  as every in-process breaker the market ships does, and the page says so.
- **A retry, a breaker or a bulkhead crate** (`backon`, `tower`'s layers,
  `failsafe`). `nest_rs_core::Backoff` is the curve every retry of the framework
  waits and `within` the one bounded wait (`container.md`); the breaker is a
  window and a phase.
- **An HTTP-date `Retry-After`.** Read as no ask, as the JWK Set fetcher reads
  it: the backoff waits instead.

**Limit, named:** the policy reads no unit deadline. Only MCP makes
`nest_rs_core::Deadline` ambient, so a retry may outlive an HTTP request, which
its edge then cuts at its deadline and answers `503`, the attempt counting in no
breaker. Every edge already hands its deadline to `nest_rs_database::run_unit`
(`UnitSpec::with_deadline`); closing the limit is `run_unit` making it ambient,
one value per unit on every edge, then each attempt bounded by the deadline's
remainder and no retry started past it.

## 2026-11-20 — no attempt outlives its unit, and the call's error says what its client is told

The owner asked for no blind spot. What the first entry named as limits is now
decided and built.

- **The unit's deadline is ambient on every edge**: `nest_rs_database::run_unit`
  makes its `UnitSpec` deadline `nest_rs_core::Deadline` inside the body, bound
  or bare — every unit but an HTTP request the edge runs with no binding — and
  the HTTP edge makes the request's at its entry, which covers that one; a unit
  nested in another keeps the sooner. Nothing is added when no deadline is set.
  An attempt then ends at the policy's timeout or the deadline, whichever comes
  first, and counts as a timeout in the breaker; no retry starts whose wait ends
  past it; a unit out of time sends nothing. A unit-stage interceptor was the
  other seam, refused: it reaches no HTTP route an app binds no unit of work
  for, and a provider every `ResilienceModule::<I>` registers would be a
  duplicate.
- **A wait the call's error asks is honoured**: `run_idempotent` takes `E: Error`
  and reads the `Retry-After` of a `Problem` in its chain, as the queue reads a
  job's.
- **A failed call answers as its error says**: `ResilienceError<E>` is `ToProblem`
  for `E: ToProblem`, the error's own problem before its chain's. The blanket
  over every `E: Error` it replaces lost a domain error's mapping in silence; a
  library's error now gets a `ToProblem` of the app's, answering `None` for a
  failure no client reads, or `map_err` inside the call.
- **An HTTP-date `Retry-After` is read**, counted from the same answer's `Date`
  — the peer's clock against itself — and from this host's when it sent none,
  through one reader every framework client calls
  (`nest_rs_reqwest::__private::retry_after`): the middleware's, the JWK Set
  fetcher's and the OAuth client's. This reverses the first entry's refusal,
  whose reason, the peer's clock, the answer's `Date` removes.
- **The middlewares' order is held by code**: `ReqwestMiddleware` marks the
  request it traces, and `ResilienceMiddleware` meeting the mark refuses it,
  unsent, saying to add it first.
- **The transitions' counter is made at the first transition**, on the meter
  installed by then, so a policy built before `OpenTelemetry::init()` counts.
- **A suite calls `nest_rs_testing::HttpsPeer`**, the scripted HTTPS peer this
  crate's and `nest-rs-reqwest`'s suites shared as two copies. The JWK Set, OAuth
  and OTLP suites keep doubles of their own: a client certificate asked, an
  answer changed mid-test, a body written in chunks, a protocol that is not
  HTTP/1 text.

## 2026-11-21 — what #7 brought

A second implementation of item 1, Just-Replicant's #7, was compared with this
one against the rules. This one stands as the base; two of #7's choices were
better and are carried over, credited on the commit that carries them:

- **A slot of the bulkhead is waited for within the unit's deadline**, as #7
  bounded its wait by the call's own, so no wait outlives the unit.
- **Retries and refusals are counted**, as #7's `PolicyStats` counted them — here
  as OpenTelemetry counters beside the transitions'
  (`nestrs.resilience.retries`, `nestrs.resilience.refusals` by
  `nestrs.resilience.refusal`), on one `Counters` per policy.

**Refused from #7:** a policy built by a builder and found in a registry by its
name (`named-instances.md`, `container.md`'s one seam); codes of its own,
`TIMEOUT` (`504`), `CIRCUIT_OPEN` and `BULKHEAD_FULL`
(`every-refusal-carries-its-class.md`); a policy with no timeout; a
`Retry-After` left unread; and a breaker on a failure rate after a minimum of
calls, which a count of failures in a window already is — twenty attempts
opening at ten is half of them after ten at least — at no new variable.

## 2026-11-21 — `#[resilient]`, the policy's decorator

A resilient call was hand-managed: inject the policy, wrap the call in a
closure, pick `run` or `run_idempotent` — the cross-cutting concern `CLAUDE.md`'s
*Identity* calls a framework defect. With the owner's decision of 2026-11-21,
**the first entry's refusal of "a method attribute beside the policy API" is
reversed**: the attribute says idempotency at the call site, where it was
refused for not being able to, since `idempotent` is the developer's own key;
and it is no second way to do one thing, the closure staying the form for what
no method is — a call needing its `CancellationToken`, or part of a method.

- **`#[resilient(field, idempotent, timeout = "…", retries = N)]`**, in
  `nest-rs-resilience-macros`, re-exported by `nest-rs-resilience` and
  `nest_rs::prelude`. The field is required and stays the method's explicit
  `#[inject]`, so the access graph refuses a missing `ResilienceModule<I>` at
  boot. `idempotent` runs the body through `run_idempotent`, else `run`, once; the
  macro never infers it. `timeout` and `retries` pin this method's own, in the
  ranges `ResilienceConfig` holds its fields to, `retries` beside `idempotent`
  alone; the breaker and the bulkhead take none.
- **The method answers `Result<T, ResilienceError<E>>`, read by its type**; its
  body is written as answering `Result<T, E>`. The body's tokens and spans are
  emitted untouched inside the attempt's closure: `move` and `async move` for a
  call run once, so it behaves as undecorated; neither when idempotent, so each
  attempt borrows the arguments again.
- **One misuse, one error.** Every helper the expansion calls is bounded on the
  answer at the return type's span, which rustc reports once per span: the
  answer probe (`ResilientAnswer`) sits on `run` and `run_idempotent`
  themselves; their bound types the async block's output only once its body is
  typed, which lets `?` and `return` infer but makes no coercion, so the body
  first returns the attempt's type under `if false` (`attempt`), never taken,
  and each answer it writes is checked against that type, coerced as the
  undecorated method's is; and `ChainedError` words an idempotent method's
  `E: Error` in the framework's sentence. The field probe (`PolicyField`) is
  spanned at the field's name, the replayed closure at `idempotent`, so
  `E0507` points at the declaration of the replay. A refused
  method is kept in the nearest form its body compiles in — run once, or typed
  against a stand-in (`refused`) that nothing runs where the receiver or the
  field is what was refused — so its body adds no error of its own.
- **A run carries its own timeout and retries**: the policy's, or the method's
  pins, through one tier-2 `ResilientMethod` a `const fn` builds. The `debug`
  lines of a decorated run name its `host` (the type's short name, read off
  `type_name::<Self>()` only when a line is filed) and its `method`; the counters
  stay by instance, a method label multiplying their series.
- **An impl half keeps another decorator on the developer's method.** `#[routes]`,
  `#[messages]`, `#[tools]` and `#[processor]` already did; `#[operations]`
  moved every attribute async-graphql does not read onto its generated field
  wrapper and is fixed with this change.

**Refused:**

- **`#[retry]`, `#[timeout]`, `#[circuit_breaker]` as decorators of their own**:
  one decorator per concern (`CLAUDE.md`, hard "no").
- **A pin for the breaker or the bulkhead**: their state is shared by every
  caller of the dependency, so one method cannot hold its own.
- **`fallback = …`**: the `Result` the method answers is the fallback, and a
  fallback would need arguments a once-run body may have consumed.
- **Naming the instance type, `#[resilient(Carrier)]`**: it needs `#[injectable]`
  to emit `Injects<S>` per field, and a host injecting one type twice under two
  spellings would stop compiling, a break inside a major.
- **No field at all**, an ambient or global lookup: the boot could no longer
  refuse a missing import, and a `static` would share one breaker across two
  apps in one process.
- **Replaying a whole unit**, a replayable `Next` (`one-execution-pipeline.md`).
- **Handing the body its `CancellationToken` or attempt number**: `run` and
  `run_idempotent` stay for that.
- **A return type wrapping the error, `Result<T, MyError>`**: such a method
  calls a decorated one and maps its error.
- **Any change to the breaker, the bulkhead or `ResilienceConfig`, and an
  outbound rate limit**: out of this change; the rate limit is a piece of work
  of its own.

**Limits, named:** the idempotent refusal of a `mut` binding or a `&mut`
parameter reads spelling, so a `&mut` behind a type alias reaches rustc's
`FnMut` sentence instead, as the rustdoc says. A method under `#[async_trait]` is
a `fn` by the time the attribute reads it, and is refused naming the native
`async fn`. A `#[scheduled]` tick answers `anyhow::Result<()>`, so it calls a decorated
method rather than being one; closing that is `#[scheduled]` converting the
error with `Into::into` as `#[hooks]` and `#[indicators]` do, a widening of its
documented answer for the owner to agree. A GraphQL field writes its `Result`
out, since `#[operations]` reads a field's answer by its spelling — `#[operations]`' own limit, stated so far for
subscriptions alone. No `demo/` use site: the demo calls no dependency it does
not control, and one is not invented for the decorator.
