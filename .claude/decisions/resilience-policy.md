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
  future bought no stack, the run's type reserving its size all the same, and
  cost an allocation and its size a second time; held in place, one holding a
  third of a 2 MiB worker's stack across an `.await` runs on that worker
  (`a_run_holding_a_large_buffer_across_an_await_runs_on_a_worker_of_tokios_default_stack`).
  A call made again holds its closure there whatever its size too: it calls the
  closure where it is held, and the run's type reserves the closure's size
  either way, so boxed past the threshold a closure bought no stack and cost an
  allocation. A call sent once holds its closure there up to that
  threshold, 16 KiB (`BOX_FUTURE_THRESHOLD`, copied in `src/call.rs`,
  `.claude/upstream/tokio.md`), and past it boxes the closure and the future it
  makes, since the closure leaves its box to be called while that future is
  built beside it on the stack: with either held in place, a call sent once
  overflowed a 2 MiB worker moving into its closure a buffer that boxing both
  runs with, three eighths of that stack
  (`a_call_sent_once_moving_a_large_buffer_into_its_closure_runs_on_a_worker_of_tokios_default_stack`).
  Its run's type still reserves the closure's size and the
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
  holding a small one in place weighed a large one's size at each step, its
  caller awaiting it taking more of the stack for each byte of the future than
  awaiting the future alone, and overflowed the worker a large one's
  twin runs on; choosing the type by the size of what it holds needs
  `generic_const_exprs`, unstable. The middleware's future is
  reqwest-middleware's box already. A decorated method's future is then its
  arguments and a run's bytes, under a kilobyte
  (`a_decorated_methods_future_holds_its_body_and_each_argument_once`), where it
  held its body and an argument taken by value more than once.

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
  worker moving into a call sent once's closure a buffer that boxing both runs
  with, and in a debug build a large one, or a small one where a type
  that may hold a large one holds it, overflowed the worker its twin runs on
  (*How each holds*, above).
- **A call made again boxing its closure past the threshold**: it calls the
  closure where it is held, and its run's type reserves the closure's size
  either way, so the box bought no stack and cost an allocation
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
  for every slot up to the window's ceiling. Closing it is
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
- **A run answering at once costs more than the first policy's**, the policy
  the 2026-11-20 and 2026-11-21 entries built, before the rate, the breaker's
  age and probes and the waiters' bound: its futures are polled by hand, holding
  the call once where the first policy's `async` fns held it twice
  (rust-lang/rust#62958), and its breaker's step is a pass, then a sent attempt
  checked against the epoch. By how much was measured against code since
  removed, with a harness never committed, so no figure of it is kept. Closing
  the first is the run written as `async` fns again once rustc holds an awaited
  future once; the second, a breaker step 7.1's switches and probes do not
  need. What a run costs now is `benches/policy.rs`'s to measure
  (`cargo bench -p nest-rs-resilience --bench policy`), a figure of the machine
  and the run it is read on, never a constant. In the dev container on
  2026-11-24 — aarch64, criterion's medians of one run, a row moving by up to
  13 ns, and one by 29, from the run before it, another build sharing the
  cores — a call answering at once took 85 ns through `run`, 85 through
  `run_idempotent`, 81 through a decorated method run once and 100 through an
  idempotent one on a policy with no optional mechanism (`run/bare`,
  `run_idempotent/bare`, `resilient/bare`, `resilient_idempotent/bare`); the
  breaker added 9 to 11 ns to each (`…/breaker`), the bulkhead up to 5
  (`…/bulkhead`) and the rate 16 to 20 (`…/rate`), and the attempt's token
  alone, built and dropped, took 16 (`token/new`). An `Ok` its method's keys
  read took no more than one the defaults read, 96 ns against 100, and 111
  under a `fallback`; an `Err` the default counts as a failure took 275 where
  one `record_if` refuses took 80, the failure's chain being said for the
  breaker's opening whether or not the policy has a breaker (`judged/bare`). A
  call an open breaker refuses unsent took 72 ns through `run` and 69 through a
  decorated method, and with a subscription to the policy's events open 83 and
  131 (`refused`).
- **A decorated method run once that takes a large argument by value** still
  needs more of a debug build's stack than its undecorated twin: the closure
  owning the argument is built in the method's frame, moved out of its box to be
  called, and its future built on the stack before it is boxed, each copy a
  debug build's stack holds a moment. One taking three thirty-seconds of a
  2 MiB worker's stack runs on that worker
  (`a_decorated_method_taking_a_large_buffer_by_value_runs_on_a_worker_of_tokios_default_stack`);
  where it overflows, against its twin, no committed test measures.
  Idempotent, borrowing its arguments, or built for release, it runs
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

## 2026-11-24 — every capability of the reference, one decorator

The owner's decision: the policy offers what `@nestjs/resilience` offers,
capability for capability, carried by `#[resilient]`, with what Rust adds.

1. **One decorator carries each capability as a key.** `#[retry]`,
   `#[timeout]` and `#[fallback]` stay refused.
2. **Five refusals of the entries above are reversed**: classifying an error
   (`retry_if`, `record_if`, and `retry_ok_if` over an answer), a fallback
   declared at the decorator (`fallback`, `fallback_if`), events readable in
   code (`ResiliencePolicy::events`), a token-bucket limiter, and the
   attempt's `CancellationToken` in a decorated body (`cancel`).
3. **The token bucket replaces the fixed window**: one way to count a rate.
4. **By default every `Err` stays a failure**; classifying is the developer's
   line, never the framework's guess.
5. **Unchanged**: the one `503`, `nest_rs_core::Backoff`'s curve, one named
   policy per dependency, state per process, no store.

**The correspondence**, read off `@nestjs/resilience` 0.0.3 (`dist/`), the
reference's word first:

| The reference | Here | Differs by |
|---|---|---|
| `@Resilience(preset)`, `@Retry`, `@Timeout`, `@CircuitBreaker`, `@Bulkhead`, `@Fallback` on a handler or a class | `#[resilient(field, …)]` on the method making the call: a provider's, or a handler of every edge but a WebSocket connection hook, which answers `()` | one decorator; per method, never per class; the field names the policy, so a missing import is refused at boot |
| `ResilienceService.preset(name)` / `create(options)`, `policy.execute(fn, { source })` | `Arc<ResiliencePolicy<I>>` injected, `run` and `run_idempotent` for a call that is part of a method | a policy is named by a type and built from `<PREFIX>_RESILIENCE__<INSTANCE>__*` or `for_root`; none is built inline; a call run by hand is told with no `host` and no `method`, a decorated method with its own |
| `ResiliencePolicy.wrap(…)`, any order | one order: a call out of time or meeting an open breaker is refused before any wait for a slot, and the rate is read there; then the bulkhead's slot, held for the whole run; then per attempt the unit's deadline, the breaker, the rate, the timeout; the fallback once the run is over | no second order |
| `retry.attempts` (total, default 3, `Infinity`) | `RETRIES` (after the first, 0 to 10, default 2, 0 for none); `retries = N` on a method, 1 to 10 | bounded: ten at most; a method wanting none drops `idempotent` |
| `retry.backoff` (`delay`, `factor`, `maxDelay`, `jitter`, or a function) | `BACKOFF_FIRST_MS`, `BACKOFF_CEILING_MS`; doubling, ±20 % | the curve is the framework's one; a `Retry-After` the dependency asks is waited whole |
| `retry.retryIf(error, attempt)` | `retry_if = F`, `F: Fn(&E) -> bool` | default: every `Err` (the reference's skips an error carrying a 4xx status); no attempt number; a timeout is sent again and read by none |
| `retry.retryOnResult(result, attempt)` | `retry_ok_if = F`, `F: Fn(&T) -> bool` | a key of any decorated method, a handler included; the retries spent, the last `Ok` answers |
| `idempotent`, else `GET`/`HEAD`/`OPTIONS` and GraphQL queries | `idempotent`, always written; `ResilienceMiddleware` reads RFC 9110 §9.2.2's methods, `QUERY` and `Idempotency-Key` | never inferred for a method |
| a retry re-runs the pipes, the inner interceptors and the handler | a retry re-runs the method's body, each attempt in a savepoint of the unit's transaction | the unit is not replayed |
| `@Timeout(d)` per attempt | `TIMEOUT_MS`, `timeout = "2s"` | every policy has one; the unit's deadline ends an attempt sooner |
| `@Signal()`, `AttemptContext.signal`, `ResilienceContext.signal` | `cancel = token` in a decorated body; the closure's argument in `run` | the attempt's future is dropped at its bound, so the token is for work handed to another task |
| `AttemptContext.attempt`, `ResilienceContext.attempt` | — | not handed: the 2026-11-21 refusal stands; a closure counts its own calls |
| `circuitBreaker.slidingWindow` `count` | `BREAKER_WINDOW` | — |
| `circuitBreaker.slidingWindow` `time` of `A` | `BREAKER_AGE_SECS` of `A` over a `BREAKER_WINDOW` of twice `minimumCalls` or more | an outcome is forgotten at its own age, not by tenths of the window; whole seconds in the environment |
| `failureRateThreshold` `T`, `minimumCalls` `M` | `BREAKER_FAILURES` of `⌈T × M / 100⌉`, the fewest failures the reference opens on | a count, read when a failure is counted: it opens at that many failures whatever the answers between them |
| `openDuration` | `BREAKER_OPEN_SECS` | — |
| `halfOpenMaxCalls` (together, judged by their rate) | `BREAKER_PROBES` (in a row, one at a time) | one failed probe opens again |
| `recordIf(error)` | `record_if = F`, `F: Fn(&E) -> bool` | — : an `Err` it refuses is counted in neither direction, and a probe hands its turn on |
| `breaker.state`, `breaker.stats` | `breaker_state()`, `breaker_stats()` (`outcomes`, `failures`, `failure_rate()` from 0 to 1) | the age is applied at the read |
| `breaker.trip()`, `breaker.reset()` | `open_breaker(duration)`, `close_breaker()` | an opening says how long |
| `bulkhead.maxConcurrent`, `maxQueue`, `queueTimeout` | `CONCURRENCY`, `CONCURRENCY_WAITERS`, `CONCURRENCY_WAIT_MS` | the slot is held for the whole run, its retries included |
| `bulkhead.active`, `bulkhead.queued`; a rejection's `reason`, `active`, `queued` | `concurrency_running()`, `concurrency_waiting()`; `Full` | a call is in one of the two reads at any instant; the refusal says neither which cause nor a count, its `debug` line's `waiters` and the reads do |
| `outboundRateLimit.limit`, `interval`, `maxWait` (token bucket) | `RATE_LIMIT`, `RATE_WINDOW_SECS`, `RATE_WAIT_MS` (token bucket) | counts attempts, a retry included; a token is waited for only when it comes before the unit's deadline; one given back goes to the next in line |
| `limiter.available` | `rate_available()` | — |
| `@Fallback(method \| fn, { handleIf })` | `fallback = method`, `fallback_if = F` | a method of `Self` alone, its signature checked: handed the method's arguments, then the error by value; default: every error (the reference's skips a 4xx); the failed attempt's savepoint is rolled back first |
| `preset.fallback` for `execute()` | the `Result` a run answers: `.or_else(…)` | — |
| `ResilienceEvents.events$`, `nestjs:resilience:<type>` channels | `ResiliencePolicy::events()`, beside the lines on `nest_rs::resilience` and the counters | per policy; bounded, a subscription behind is told how many it missed; a send never runs a subscriber; a line, an event and a counter each say their own fact (below) |
| `retry`, `timeout`, `circuit-open` / `-half-open` / `-closed`, `circuit-rejected`, `bulkhead-rejected`, `rate-limited`, `fallback` | `ResilienceEvent`'s `Retry`, `Timeout`, `Transition`, `Refusal`, `Fallback` | one transition and one refusal event, each saying which; the error as its chain in text, never the value |
| `ResilienceService.circuitBreaker(name)`, `bulkhead(name)`, `circuitBreakers()`, `bulkheads()`, for an app to export | each policy injected by its type; its reads observed on the meter under `opentelemetry` | none is looked up by a name or enumerated; the export is built |
| `ResilienceModuleOptions.defaults`, merged under every preset and decorator | `ResilienceConfig`'s defaults; `for_root` pins a base per instance, the environment overlays it | no defaults shared across policies |
| `ExecuteOptions.signal`, the caller's own cancellation | the caller drops the run's future | an attempt in flight is dropped with it, counted in neither direction before its bound and a timeout past it; a slot, a token or a probe's turn it held is given back |
| `mapErrors`: `504` / `503`, `RpcException`, `WsException`, `GraphQLError`; `TIMEOUT`, `CIRCUIT_OPEN`, `BULKHEAD_FULL`, `RATE_LIMITED`; `mapErrors: false` | `ToProblem`: `503` `UNAVAILABLE` on every edge, `Retry-After` where known; the variant tells them apart in code | one class; no switch — a method answering otherwise maps the `ResilienceError` it holds |
| `isClientError()` as the default filter | none: every `Err` is a failure; `ResilienceMiddleware` reads statuses | the developer classifies |
| `executeObservable`, committed once a value is emitted | an `#[sse]` route and a `#[subscription]` run the call opening the stream | — |
| `ResilienceContext.run(scope, fn)` | the unit's ambient `Deadline` | no ambient attempt |
| — | the unit's deadline bounds every attempt and wait; a `Retry-After` read in seconds or as an HTTP-date; a queue job honouring a refusal's wait; `ResilienceMiddleware` on the app's reqwest client, its body covered; every key and value checked by the compiler | what this one adds |

**How it is built:**

- **One judge, two axes, read once per answer**, where the attempt settles
  and before the breaker's lock: `record_if` alone says whether an `Err` is a
  failure, `retry_if` and `retry_ok_if` alone whether an idempotent run sends
  the answer again. A timeout and a panic are failures no predicate reads; a
  predicate that panics is the call panicking — one failure, the attempt's
  savepoint rolled back, then it unwinds on. A run sent once reads `record_if`
  alone. A method writing no key is judged by defaults that weigh nothing in
  its run.
- **A predicate reads `self` and the method's arguments**, owned ones
  included: wherever a key that reads them is written the attempt lends the
  arguments, `record_if` on a method sent once as `idempotent` and `fallback`
  do, and `fallback_if` is asked before the fallback takes them.
- **An `Err` `record_if` refuses is counted in neither direction**, the
  reference's ignored outcome: the window does not hold it, and as a probe's
  answer it hands the turn on, with no line, no transition counted and no
  event. Counting it as an answer was the first build, and the one place the
  proof below found this breaker the laxer. Half-open, such answers neither
  close nor reopen it: it stays half-open, one call at a time, until an `Ok`
  or a counted `Err`, as the reference's does.
- **An answer counts in the breaker only in the phase its attempt was let
  through in**: one sent before an opening counts nowhere once the breaker has
  closed again, by its probes as by `close_breaker()` — the reference's
  generation. The proof found the other rule the laxer: successes in flight
  across a cycle pushed a fresh failure out of the new window.
- **Nothing is told under the breaker's lock.** A transition is decided there
  and its line, its count and its event come out once the lock is let go, in
  the order the transitions took effect, each once, one thread telling at a
  time and a thread finding another telling leaving its own to it. A layer, a
  meter or a subscription's waker reading or switching the breaker from inside
  one returns: the transitions' counter built under that lock, while a metrics
  export ran the breaker's observed reads under its own, was a deadlock at a
  process's first transition. Across threads a call can meet the new phase,
  and have its refusal told, before the transition that caused it.
- **The fallback answers after the run**, for the call's own error once its
  attempts are spent and for a refusal that sent nothing; `fallback_if` is
  asked first. It is a method of `Self` named by the key, handed the method's
  arguments under their names and then the error by value, so every attempt
  lends the arguments: a `mut` binding, a `&mut` and a parameter bound by a
  pattern are refused under `fallback`. The failed attempt's savepoint is
  rolled back before it runs, and where the filter refuses;
  `ResilienceError::Unsettled` never reaches it, the unit being unable to
  commit what a fallback would write. Its future is held as an attempt's is,
  in place in a release build and boxed in a debug one; a parameter under a
  `#[cfg]` is handed under it.
- **A key's value is bounded by a trait of the key's name**
  (`RetryIf`, `RetryOkIf`, `RecordIf`, `FallbackIf`, `Fallback`, in the
  tier-2 seam), whose `#[diagnostic::on_unimplemented]` is the key's sentence:
  a value of another shape — a function of another arity, a constant — is one
  error at the value, and a closure taking no argument or several, or written
  `async`, the decorator's own refusal at the closure.
- **`cancel = token` binds the attempt's `CancellationToken`** as a local of
  the body, never a parameter: the signature stays what callers call. A name
  an argument already holds is refused, and so is the key beside an argument a
  macro's pattern binds, whose names no attribute reads.
- **The rate is a token bucket**, the generic cell rate algorithm (ITU-T
  I.371) in whole ticks of `1 / RATE_LIMIT` nanosecond: one instant, exact for
  every value the config takes. Attempts finding no token wait in arrival
  order, each told its wait once; a reservation given back moves the line up
  one token.
- **`ResiliencePolicy::events()` opens a subscription**, bounded at 256 unread
  events, past which it is told how many it missed (`Lag`). With none open an
  event is not built
  (`an_event_is_built_only_while_a_subscription_is_open`); what an open one
  costs a call refused unsent is the `refused` group of `benches/policy.rs`.
- **A line, an event and a counter each say their own fact.** A policy files
  ten lines: at `debug` a retry (`attempt failed; retrying the call`, and
  `attempt answered; retrying the call` for an `Ok` sent again), a retry given
  up for a wait past the ceiling or for the unit's deadline, a savepoint that
  could not be settled, a wait for a token, a call the bulkhead or the rate
  refuses and a fallback answering (`the call ended on an error; its fallback
  answers`); at `warn` the breaker's transition. A call an open breaker
  refuses and a run ending on a timeout are counted and told, and file no line
  of their own: one per refused call is the noise the 2026-11-22 entry
  refused. A retry's line and event are told as it is decided, its counter
  counts a call sent again; a `Transition` carries `from`, `to` and `manual`,
  what opened the breaker being on the `warn` alone.
- **The reads sit beside `breaker_state()`** — `breaker_stats()`,
  `concurrency_running()`, `concurrency_waiting()`, `rate_available()` — and
  under `opentelemetry` are observed as up-down counters, additive across
  replicas as the pool's are, registered when the policy is built and again
  at its first run on a meter provider installed since. The
  breaker's window is one instrument by `outcome`, both series from one read,
  so no export says more failures than outcomes.
- **The time window and the probes are covered**:
  `tests/integration/breaker.rs` ports the reference's breaker as a model and
  drives both through the same timed traces, calls in flight together
  included. Under the table's mapping the two read the same call for an
  outcome past its age, a quiet spell, a burst, probes answering, a probe
  failing and an answer arriving across a cycle; they answer alike until their
  first difference, and at that difference this one is the stricter — a
  failure both sent opens this breaker alone (a count against a rate or a
  minimum of calls, an outcome dropped at its age against its bucket's, a
  failed probe opening at once), or this breaker refuses beside its probe a
  call the reference sends as a probe of its own. That is sampled on
  generated traces, not proved. Past the first difference the two cycles are
  out of step by up to the open time: having opened first, this one also
  probes and closes first, and then sends calls the reference still refuses.
  A `BREAKER_WINDOW` under twice `minimumCalls` forgets by count failures the
  reference's rate still reads, and the test pins it.

**Superseded above:** the fixed window and its reset (2026-11-22: the rate's
decision, its three limits and the refusal of "a second way to count a
rate"); the refusals of `fallback = …`, of a key classifying errors, of an
in-code stream of events and of handing a decorated body its
`CancellationToken`; "every `Err` is the dependency failing" now reads "by
default"; the limit of a `#[resilient]` inside a `#[cfg_attr]` on an
operation, which is split out and kept on the method; "each forgets the
window and starts an epoch" of a switch by hand, an opening by a failure
starting one too; "where a method run once moves them" of the idempotent
`Send` limit, a method sent once under `record_if` or a `fallback` lending
its arguments as well; "every transition files its line under the breaker's
lock" and the limit it left, a transition being told once the lock is let go.

**Refused:**

- **The attempt number**, in a body, a predicate or a fallback: the
  2026-11-21 refusal stands.
- **A threshold as a rate after a minimum of calls, and probes sent together
  and judged by their rate**: the count and the series in a row are the
  stricter at the first difference, and the mapping above reaches the
  reference's cases.
- **A class-level form and a preset merged field by field**: a policy is one
  named instance, a method names it.
- **A fallback that is a closure or a free function**: it would have nowhere
  to take the method's arguments from, and no signature to check.
- **A label on a call run by hand** (`source`): a method wanting its name in
  an event is decorated.
- **A burst setting and a bound on the attempts waiting for a token**
  (`RATE_BURST`, `RATE_WAITERS`): no caller, and each a new name.

**Limits, named:**

- **The default breaker is not the reference's default mapped**: ten failures
  of twenty, where the mapping of the reference's 50 % after ten calls gives
  five. A deployment moving a default breaker sets `BREAKER_FAILURES`.
- **A bucket left idle sends its whole limit at once**, then a token at a
  time: up to twice the limit within one window's length, as the reference's.
  A dependency counting every request of the last window takes half its limit.
- **A token keeps its instant when its attempt is woken late**, so two
  attempts sent after a wait can go out closer than the interval by that lag.
  Checking again at the send would refuse an attempt after its whole wait.
- **The rate read before a slot's wait is advisory, in tokens**: it refuses at
  once only a call whose token, behind the line as it stands, lies past
  `CONCURRENCY_WAIT_MS` plus `RATE_WAIT_MS` from its arrival or past its
  unit's deadline. A call may still wait for a slot and be refused by the rate
  after, and one refused at once might have been served by a reservation given
  back since. Closing the first is reserving a token before the slot, for a
  call that may never get one.
- **A body consuming or changing an argument its `record_if` reads** is
  rustc's own error at the key — `E0505` consumed, `E0502` bound `mut` and
  changed, `E0500` written through a `&mut` — each with its remedy in the
  key's rustdoc: a predicate's tokens do not say which argument it reads, so
  refusing every `mut` there would refuse valid methods. A method sent once
  that lends its arguments and takes one that is
  not `Sync` by value is refused as not `Send` in rustc's words, naming no
  key; pointing it at the key is the run written as an `async fn`, which
  holds its call twice.
- **A predicate closure whose body answers no `bool`** is rustc's `E0308` at
  its body: what a body answers is the type checker's to read.
- **A fallback taking other arguments** is rustc's `E0308` at the `fallback`
  key or `E0061` at the fallback's name, no sentence of ours leading there; one
  that is no method of `Self` is its `E0599`, and one consuming an argument an
  attempt lends its `E0382`.
- **A refusal reported at the method's answer carries rustc's own label and
  note of an attribute's expansion**: every token the expansion writes there
  is the expansion's, which is what keeps `clippy::large_futures` silent on an
  `.await` the developer did not write. An answer a `macro_rules!` opens on
  its own `$crate`, refused, is told twice.
- **The thread making a process's first transition waits for a metrics export
  in progress**, holding no lock of the breaker: the transitions' counter is
  made at its first record. A subscriber panicking out of a transition's line
  loses that transition's count and event, and one blocking there for good
  keeps the turn, the transitions after it queued and untold; no call on the
  breaker waits for either.
- **A refusal reaching a tick, a listener or a shutdown hook is that unit's
  failure**, filed at `error` by its edge — a tick's at every occurrence while
  the breaker is open. An init hook's fails the boot naming the hook, the
  chain in the error `init` returns and on no line. A refused readiness
  indicator answers down with the chain in a `warn`, and a decorated liveness
  one would have the orchestrator restart the process for a dependency's
  outage. A `fallback` is how such a unit answers instead.
- **A policy whose first run precedes `OpenTelemetry::init()`** never reports
  its observed instruments, and a counter first recorded before it stays on
  the meter that records nothing. Closing it is a hook from the exporter's
  crate.
- **`ResilienceEvents` offers `next()` alone**, no `Stream`: it would bring
  `futures-core` into the port.
- **`BREAKER_AGE_SECS` is whole seconds** in the environment; a window under a
  second is `for_root`'s.
