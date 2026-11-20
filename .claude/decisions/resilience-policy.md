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
