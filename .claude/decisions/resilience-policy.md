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
- **An impl half keeps `#[resilient]` on the developer's method**, read by its
  name. `#[routes]`, `#[messages]`, `#[tools]` and `#[processor]` leave any other
  decorator there too; `#[operations]` hands an operation's other attributes to
  the field it generates, where 7.0 placed them.

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
out, since `#[operations]` reads a field's answer by its spelling: a decorated
field answering any other is refused at its return type, in one sentence, as a
subscription's aliased `Result` is. No `demo/` use site: the demo calls no
dependency it does not control, and one is not invented for the decorator.

## 2026-11-22 — the rate, the breaker's age, probes, read and switch, the waiters' bound, and a decorated tick

The owner's decision of 2026-11-21, each point of it built:

1. **The policy gains** an outbound rate limit (`RATE_LIMIT`, `RATE_WINDOW_SECS`,
   `RATE_WAIT_MS`), an age bound and a probe count on its breaker
   (`BREAKER_AGE_SECS`, `BREAKER_PROBES`), a bound on its bulkhead's waiters
   (`CONCURRENCY_WAITERS`), a read of the breaker's state (`breaker_state()`)
   and a manual open and close (`open_breaker`, `close_breaker`).
2. **All of a policy's state stays in its process**: no store, no port, no Redis
   or Valkey binding, no new crate, no new dependency, and nothing that makes an
   app run a service it does not already run.
3. **A scheduled tick may answer any error that converts into `anyhow::Error`**:
   the widening the 2026-11-21 entry's limits left for the owner, agreed and
   built.
4. **The public names are the ones the decision names**: the six settings,
   `breaker_state()`, `open_breaker`, `close_breaker` and
   `ResilienceError::Limited`. It names none for the read's enum or the
   switch's error, so `BreakerState` and `BreakerError`, its `Off` and
   `OutOfRange` included, are the build's, not the owner's: the breaker's word,
   then what the type is, as `ResilienceConfig` and `ResilienceError` take the
   crate's. `BreakerState`'s variants spell the three states the decision
   names — closed, open with the time left, half open — `Open`'s time named
   `retry_after`, as `ResilienceError::Open` names it.
5. **The refused list below stays refused.**
6. **No use site in `demo/`**: the demo calls no dependency it does not control.

How each holds:

- **The rate counts the attempts sent, over a fixed window** the first one
  counted opens, as the throttler's stores count, in their words (`limit`,
  `window`): every attempt, a retry included, through `run`, `run_idempotent`,
  `#[resilient]` and the middleware. An attempt meets the unit's deadline, the
  breaker, then the rate, so an open breaker counts nothing, and an attempt the
  rate refuses is never sent, settles nothing in the breaker and hands a probe's
  turn on. Past the limit it waits for the reset within `rate_wait` and the
  unit's deadline — its waits never adding up past `rate_wait`, its slot of the
  bulkhead held, its timeout started at its send — or ends the run `Limited`,
  the `503` with the time to the reset, never attempted again. No limit builds
  no rate.
- **The breaker forgets an outcome past its age**, counted from its answer,
  each time the window counts one; unset, it stores no instant and reads no
  clock to count. **Its probes are
  a series**, one at a time: a failed probe opens it again for its whole open
  time and the series starts over, a probe cut at its timeout or its unit's
  deadline has failed, whichever of the policy's timer and the edge's cut is
  polled first, and one never sent, or dropped by its caller before then,
  counts as neither. A
  `BREAKER_FAILURES` above what a rate lets answer within the age of one
  another, each attempt counted at the timeout's ceiling
  (`Rate::most_sent_within`), is refused at boot.
- **The read and the switch.** `breaker_state()` answers what a call arriving
  now meets, `None` for a policy with no breaker, said by type. `open_breaker(d)`
  replaces any open time with `d` from the call, and `close_breaker()` closes;
  each forgets the window and starts an epoch, so an attempt admitted before it
  changes nothing, files the transition's `warn` marked `manual` — the owner's
  word, since `asked` is the dependency's on this target — and counts it marked
  `nestrs.resilience.breaker.manual`. `BreakerError::Off` and
  `OutOfRange { most }` answer what a switch cannot do, never clamped and never
  a silent no-op. Every transition files its line under the breaker's lock, so
  no switch comes between a transition and its line.
- **A call refused beside a probe is told what is left of that probe's own
  bound** — its run's timeout, a method's pin included, under its unit's
  deadline, after its wait for the reset — and a millisecond past it, where the
  one bound the breaker was built with told the wrong wait.
- **A place to wait is taken in one step**, `Semaphore::try_acquire`, and given
  back by its permit's `Drop`; with every place taken a call is `Full` at once,
  its line carrying the `waiters` it met. A call finding no free slot reads the
  rate before it waits, a read that counts nothing.
- **An idempotent run is `Send` for an answer that is not `Sync`**: it decides
  whether to send again before its backoff and holds nothing of the answer
  across it, so an `#[sse]` route and a `#[subscription]` run under `idempotent`
  too.
- **The tick (decision 3).** `TickAnswer`, a type probe in
  `nest_rs_schedule::__private`, takes `Result<(), E>` for every
  `E: Into<anyhow::Error>` and `!`, each impl `do_not_recommend`, every answer
  read by its type, `!` included. The expansion hands the call's own future to
  `answered`, which boxes `Answered` over it, the future pinned in place and held
  once, at the size 7.0 boxed the run at; it binds no answer and writes no lint
  attribute, so no lint level a crate sets refuses a tick 7.0 compiled, in
  edition 2021 as in 2024. The family keeps two bounds: `#[hooks]`,
  `#[indicators]` and a tick take
  `E: Into<anyhow::Error>`; `#[on_event]` and `#[process]` take
  `E: Into<Box<dyn Error + Send + Sync>>`, and `#[process]` alone has a
  `ToProblem` tier beside it. The class between them is a difference stated, not
  closed by a tier for a hypothetical user: a boxed error, a `String` and a
  `&'static str`, which `#[on_event]` and `#[process]` take and a tick refuses,
  and a type that is no `std::error::Error` but converts into `anyhow::Error`
  through a `From` of its own, which a tick takes and they refuse.
- **A decorated body is checked against its attempt's type**: it opens, after
  its leading items, on a `return` of that type never taken
  (`__private::attempt`), so it makes every coercion the undecorated method
  makes, run once and idempotent alike. rustc refused the other candidates: a
  `let` ascribed with the type misses an early `return`, and an async closure
  declaring it coerces nothing (`.claude/upstream/rustc.md`). A policy setting
  written before the field is refused as one after it, told where it is set.
- **`#[operations]` places an operation's attributes as 7.0.0 did**, on the
  wrapper it generates, but for what the developer's method keeps: its docs, its
  `allow`s, its `expect`s and its `#[cfg]`s, as 7.0.0 kept them, and
  `#[resilient]`. The wrapper carries each `#[expect]` as an `#[allow]` of the
  same lints and reason, so the expectation is checked on the method alone;
  7.0.0 handed it to the wrapper too, where it went unfulfilled. `#[resilient]` is
  read by its path's last segment in a build where any crate enables the
  capability — `nest-rs-graphql-macros`' `resilience` feature, which
  `nest-rs-graphql`'s enables and the umbrella's `resilience` forwards weakly,
  so no app writes a manifest line. Cargo unifies a package's features across a
  build, so a dependency, or a workspace member built beside a crate, turns the
  reading on for that crate too. A build without it places an attribute of that
  name on the wrapper like any other, as 7.0.0 placed an app's own, and refuses
  nothing 7.0.0 compiled; a crate with an attribute of its own by that name
  imports it under another name, which goes on the wrapper in every build.
- **A bound's reason holds for every value its siblings take**: it says what its
  own mechanism does to a call or lets happen, true for every value it refuses,
  every value its siblings take, every traffic and every client a policy wraps,
  never an end another mechanism or the traffic decides, and reads each number it
  states off a constant. Two lines judge every floor: no timer is told less than
  it can honour, and no line says `0` for a duration that is not zero. So the
  open time and the age, read off the clock and told to no timer, take any value
  above zero in code, as a manual opening does, and so does the rate's window,
  read off the clock and told to a timer only as the time left to its reset: a
  wait computed, not set, which tokio's timer waits at least, as it waits a
  backoff jittered below its floor. The five `_MS` settings, each a timer's wait
  or its bound, take a millisecond at least in code as from the variable; the
  two waits are an `Option<Duration>`, `None` for none; and a line rounds every
  duration up to whole milliseconds.
- **A queue job refused by an open breaker or a spent rate** waits at least
  the refusal's `Retry-After` before its retry, up to the queue's hour.
- **A run holds its call once.** The runs of `run`, `run_idempotent`, a
  decorated method and the middleware are futures polled in place, holding the
  closure until an attempt is let through and then the future it made, where an
  `async` fn or block awaiting a future it was handed holds it twice
  (rust-lang/rust#62958); the one `async fn` left, a decorated idempotent run's,
  holds its closure of borrows twice, so rustc's error for a future that is not
  `Send` still points at `idempotent`. A release build holds each attempt's
  future where the run is, whatever its size: boxed past tokio's threshold, a
  future bought no stack, the run's type reserving its size all the same — held
  across an `.await`, futures up to 1,984 KiB, the largest probed, run on a
  2 MiB worker either way — and cost an allocation and its size a second time.
  A call made again holds its closure there whatever its size too: it calls the
  closure where it is held, and the run's type reserves the closure's size
  either way, so boxed past the threshold a closure bought no stack — moving a
  buffer in, it runs at 672 KiB and overflows from 704 KiB, boxed or held — and
  cost an allocation. A call sent once holds its closure there up to that
  threshold, 16 KiB (`BOX_FUTURE_THRESHOLD`, copied in `src/call.rs`,
  `.claude/upstream/tokio.md`), and past it boxes the closure and the future it
  makes, since the closure leaves its box to be called while that future is
  built beside it on the stack: with either held in place, a call sent once
  moving 704 KiB into its closure overflowed a 2 MiB worker, where boxing both
  runs 992 KiB. Its run's type still reserves the closure's size and the
  future's beside the two boxes, in the one place it holds either in when they
  fit. So a call answering at once allocates nothing but its attempt's token;
  the decision is a constant per type, `AutoBox::<T>::SHOULD_BOX` as tokio's
  is, so only the branch taken is built. No step of a run moves what it holds:
  each writes its fields where they are, and what a run waits on besides its
  call — its attempt's bound, and only when they come a slot, the rate's reset,
  a backoff — shares one place. As it answers, a run lets its closure go, with
  what it captured, and its attempt's bound, whose timer would otherwise wake
  its task once more: a run whose answer is kept unread holds neither. A debug
  build boxes both whatever their size, a box of what holds nothing allocating
  nothing: it copies a value's whole type at each move, and a type that may
  hold a future in place has the size of the largest it may hold, so a run
  holding a small one in place weighed a large one's size at each step — a
  run's caller awaiting it took 3.1 times the future's size on the stack, where
  awaiting the future alone takes 2.1 — and overflowed the worker a large one's
  twin runs on; choosing the type by the size of what it holds needs
  `generic_const_exprs`, unstable. The middleware's future is
  reqwest-middleware's box already. A decorated method's future is then its
  arguments and a run's bytes, under a kilobyte, where it held its body twice
  and an argument taken by value seven times.

**Refused:**

- **A store, a port or a vendor binding for any of the state**: decision 2.
- **`fallback = …` in the decorator.** A body run once has consumed the
  arguments a fallback needs; the `Result` is the fallback.
- **A key classifying errors** (`record_if`, `retry_if`). An answer is `Ok`, a
  failure is `Err`.
- **`#[retry]`, `#[timeout]`, `#[circuit_breaker]`, and a pin for the breaker,
  the bulkhead or the rate**: one decorator per concern, and a mechanism's state
  is every caller's. Every setting but `timeout` and `retries` is refused at a
  method naming the variable and the field that set it.
- **A code of its own, a `504` or a `429`.** One class; a reply names no
  internal mechanism.
- **An in-code stream of events.** A transition is a `warn` and a counter.
- **A second way to count a rate** (token bucket, sliding window): the window
  is the throttler's, one way to count, and a dependency counting a sliding
  window is given half its quota instead.
- **Inbound idempotency.** It is ROADMAP item 5, another capability, waiting on
  an owner's decision.
- **Every attribute of an operation kept on the developer's method**: it
  refuses 7.0 programs whose attribute names a parameter the field leaves out —
  `#[tracing::instrument(skip(self, ctx))]` beside a `Scoped<T>`, which is no
  `Debug` — a break inside a major. `#[resilient]`, new in 7.1, is the one kept
  there, in a build where any crate enables its capability, which no 7.0 build
  can: 7.0 has no such feature.
- **`#[resilient]` read by its full path**: the docs import it under its short
  name, as every decorator is.
- **A call sent once holding its closure in place whatever its size, or its
  future in place beside its boxed closure, or a debug build holding either in
  place at all**: in a release build each of the first two overflowed a 2 MiB
  worker from 704 KiB moved into a call sent once's closure, where boxing both
  runs 992 KiB, and in a debug build a large one, or a small one where a type
  that may hold a large one holds it, overflowed the worker its twin runs on
  (*How each holds*, above).
- **A call made again boxing its closure past the threshold**: it calls the
  closure where it is held, and its run's type reserves the closure's size
  either way, so the box bought no stack — moving a buffer in, it ran at
  672 KiB and overflowed from 704 KiB, boxed or held — and cost an allocation
  (*How each holds*, above).

**Limits, named:**

- **Each replica's own**: its breaker and a switch of it, its bulkhead and its
  waiters, its rate's count. A limit set for the whole app is divided by the
  replicas. Closing any of it is a store, refused by decision 2.
- **A fixed window lets up to twice the limit through** within one window's
  length around a reset. Closing it is a second way to count, refused above.
- **The rate counts attempts, not requests**: a redirect the app's reqwest
  client follows goes out inside the attempt that met it, ten at most on
  reqwest's default policy, and a second request in a decorated body or a `run`
  closure inside that body's one attempt, so a dependency counting every request
  can be sent more than `RATE_LIMIT` in a window. A client built with
  `redirect::Policy::none()` answers a redirect as it came; closing it for a
  redirect is the middleware following redirects itself on a client following
  none, overriding the policy the app chose. A body sends each request from a
  decorated method or a `run` closure of its own; nothing else closes it, the
  policy seeing none of the requests a body sends.
- **A wait for the window's reset holds its slot of the bulkhead**, as the
  owner's decision of 2026-11-21 asks. Closing it is the owner's: a wait giving
  its slot up and queuing again after the reset.
- **The rate read before a slot's wait is advisory**: a window others spend
  after it still costs the call its wait. Closing it is counting the attempt
  before the wait, which counts attempts never sent.
- **An age costs memory**: a slot holds its outcome's instant beside its byte,
  24 bytes on 64-bit Linux, about 240 KB at the window's ceiling. Closing it is
  a narrower instant per slot, which no measurement has asked for.
- **The boot's check of an age under a rate** counts each attempt at the
  timeout's ceiling and each timeout as firing on time: a policy whose attempts
  all end sooner can still be built with a breaker that never opens, and a
  runtime stalled past a timeout can let more failures answer within the age
  than it counts. Closing the first needs the policy to know at boot what its
  methods pin; the second, a margin that refuses configurations that work.
- **A transition's line is filed under the breaker's lock**: a subscriber or a
  meter reading or switching that breaker from inside the line waits for good;
  none of the framework's does. Closing it is filing lines after the lock in an
  order the epoch keeps.
- **Each attempt allocates its `CancellationToken`**, which its call is handed:
  tokio-util's token is shared through an `Arc`, and every run of the policy has
  paid it since its first commit. Closing it is a call asking for its token,
  which `run`'s signature, handing one to every call, does not let a policy
  know.
- **A debug build boxes a run's call and each attempt's future**, an allocation
  each for one that holds anything. Closing it is a type chosen by the size of
  what it holds, which waits on `generic_const_exprs`
  (`.claude/upstream/rustc.md`). A release build's call sent once whose
  closure is past the threshold boxes the closure and its future, and its run's
  type still reserves the closure's size and the future's beside the boxes, in
  the one place it holds either in when they fit, so such a run weighs the
  larger twice where its task is held; the same type closes it.
- **A run answering at once costs 15 to 19 ns more than the first policy's**,
  the policy the 2026-11-20 and 2026-11-21 entries built, before the rate, the
  breaker's age and probes and the waiters' bound. In release on one machine,
  medians of three interleaved rounds, on a policy with none of those settings:
  in a binary timing `run` alone, a call answering at once ran in 77.9 ns,
  where the first policy ran it in 62.2 and each attempt's box had taken it to
  112.9, and with the rate set in 96.3, where the box had taken it to 125.7; in
  a second binary timing four paths, `run` took 88.5 against the first policy's
  70.0, `run_idempotent` 93.0 against 74.3, a decorated method 89.7 against
  74.5 and a decorated idempotent one 99.1 against 81.1. With the attempt's
  token, the panic's containment and the deadline's read stripped from both,
  the first policy ran it in 35.4 ns and this one in 52.5: 13.5 of those 17.1
  is the run itself, measured with no breaker in either (43.5 against 30.0) —
  its futures polled by hand, holding the call once where the first policy's
  `async` fns held it twice (rust-lang/rust#62958) — and 3.6 the breaker's
  step, 9.0 here against 5.4 there, a pass then a sent attempt checked against
  the epoch; the three stripped cost 1.4 ns less here (25.4 against 26.8), so
  the parts add up to the 15.7 through `run`. Closing the first is the run
  written as `async` fns again once rustc holds an awaited future once; the
  second, a breaker step 7.1's switches and probes do not need.
- **A decorated method run once that takes a large argument by value** still
  needs more of a debug build's stack than its undecorated twin: the closure
  owning the argument is built in the method's frame, moved out of its box to be
  called, and its future built on the stack before it is boxed, each copy a
  debug build's stack holds a moment, so on a 2 MiB worker it runs at 224 KiB
  and overflows from 256 KiB, where its twin runs at 384 KiB and overflows from
  416 KiB. Idempotent, borrowing its arguments, or built for release, it runs
  where its twin does. Closing it is a body's future built where the run holds
  it, which safe Rust cannot write.
- **An app's own attribute named `resilient` on an operation** is read as the
  framework's in a build where any crate enables the resilience capability:
  Cargo unifies a package's features across a build, so a dependency, or a
  workspace member built beside the crate, turns it on for a crate that never
  asked. The attribute then goes on the developer's method instead of the field,
  the operation refused unless it spells its `Result<…>` out: there it fails to
  compile, or compiles and applies to the method alone with no error or warning,
  an instrument then spanning the method's body and not the field's guards and
  pipes. The reading is by the path's last segment, so a full path ending in
  `resilient` is read too; the crate imports its attribute under another name,
  which goes on the field as 7.0.0 placed it. Closing it is reading the
  decorator by its path, which its short-name import defeats.
- **An attribute of an operation reading the method's own `self`** —
  `#[tracing::instrument(fields(depot = self.depot.0))]` — does not compile, as
  in 7.0: it goes on the field `#[operations]` generates, whose `self` is
  another type. Closing it is keeping the attributes on the method, a break only
  8.0 can make (ROADMAP § 2).
- **A `#[resilient]` inside a `#[cfg_attr]` on an operation** goes on the field
  with the other attributes, `#[operations]` reading it by its name only where
  it is written plain, and does not compile there while its condition holds.
  Closing it is splitting a `#[cfg_attr]` by entry, as the `#[cfg]`s inside one
  are already unfolded.
- **An idempotent method's future is `Send` only where each argument it takes
  by value is `Sync`**: every attempt borrows the arguments across its waits,
  where a method run once moves them, and an async closure owning them still
  lends each attempt a shared borrow. Closing it is each attempt owning a clone
  of its arguments, a `Clone` the method would declare, or an exclusive borrow,
  which would let a body change an argument it binds without `mut`.
- **The middleware's resend is held `Send` by its trait**, reqwest-middleware's
  boxed `Send` future, and its answer, reqwest's `Response`, is `Sync`: no test
  can give it one that is not. Closing it waits on a middleware answering
  another type.
- **The family's two bounds differ** by the class above. Closing it is a tier in
  `nest-rs-events` and `nest-rs-queue`, refused for a hypothetical user.
- **A tick's error borrowing its host is refused in rustc's words** at
  `#[scheduled]`: `E0716` for an `async` tick, naming the return type of the
  run's async block, and `E0515` for one that is not, naming no type. No
  `#[diagnostic]` reaches a lifetime error on stable. Closing it waits on rustc
  (`.claude/upstream/rustc.md`).
- **A subscription's stream item, a resource's answer and an entity's answer
  cannot name the method's lifetime**, as before this change: async-graphql's
  derive reads a stream's item outside the method, the mime probe a resource's
  answer, and an entity's claim its answer's `type_name`. Closing it is upstream
  for the first (`.claude/upstream/async-graphql.md`), and the probe and the
  claim moved inside the method's generics for the other two.
- **A stream's items run outside the policy**: on an `#[sse]` route or a
  `#[subscription]`, it runs the call opening the stream. A stream calling the
  dependency per item calls a decorated method for each; nothing else closes it,
  an item being no call.
- **A WebSocket connection hook answers `()`** and carries no `#[resilient]`:
  it calls a decorated method and handles its error. Closing it is a hook
  answering a `Result`, which `#[messages]` does not take.
