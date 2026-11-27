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
  the field it generates, where the published release placed them.

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
  once, at the size the published release boxed the run at; it binds no answer and writes no lint
  attribute, so no lint level a crate sets refuses a tick the published release compiled, in
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
- **`#[operations]` places an operation's attributes as the published release did**, on the
  wrapper it generates, but for what the developer's method keeps: its docs, its
  `allow`s, its `expect`s and its `#[cfg]`s, as the published release kept them, and
  `#[resilient]`. The wrapper carries each `#[expect]` as an `#[allow]` of the
  same lints and reason, so the expectation is checked on the method alone;
  the published release handed it to the wrapper too, where it went unfulfilled. `#[resilient]` is
  read by its path's last segment in a build where any crate enables the
  capability — `nest-rs-graphql-macros`' `resilience` feature, which
  `nest-rs-graphql`'s enables and the umbrella's `resilience` forwards weakly,
  so no app writes a manifest line. Cargo unifies a package's features across a
  build, so a dependency, or a workspace member built beside a crate, turns the
  reading on for that crate too. A build without it places an attribute of that
  name on the wrapper like any other, as the published release placed an app's own, and refuses
  nothing the published release compiled; a crate with an attribute of its own by that name
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
  refuses programs the published release compiled whose attribute names a parameter the field leaves out —
  `#[tracing::instrument(skip(self, ctx))]` beside a `Scoped<T>`, which is no
  `Debug` — a break inside a major. `#[resilient]`, new in this line, is the one kept
  there, in a build where any crate enables its capability: a crate asking for
  it, which none written against the published release does, or for the umbrella's `full`, which
  takes every capability and which such a crate may write.
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
  future once; the second, a breaker step this capability's switches and probes do not
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
  the umbrella's `full` does, and Cargo unifies a package's features across a
  build, so a dependency, or a workspace member built beside the crate, turns
  it on for a crate that never asked. The attribute then goes on the
  developer's method instead of the field, the operation refused unless it
  spells its `Result<…>` out: there it fails to compile, or compiles and
  applies to the method alone with no error or warning, an instrument then
  spanning the method's body and not the field's guards and pipes. The reading is by the path's last segment, so a full path ending in
  `resilient` is read too; the crate imports its attribute under another name,
  which goes on the field as the published release placed it. Closing it is reading the
  decorator by its path, which its short-name import defeats.
- **An attribute of an operation reading the method's own `self`** —
  `#[tracing::instrument(fields(depot = self.depot.0))]` — does not compile, as
  in the published release: it goes on the field `#[operations]` generates, whose `self` is
  another type. Closing it is keeping the attributes on the method, a break only
  a major can make (ROADMAP § 2).
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
| `retry.retryIf(error, attempt)` | `retry_if = F`, `F: Fn(&E) -> bool` | default: every `Err` (the reference's skips a client error: an `HttpException` under `500`, and any other error carrying a 4xx status but `408` and `429`); no attempt number; a timeout is sent again and read by none; the reference never retries a rejection (an open breaker, a full bulkhead, a spent rate), where one an inner policy answers is here an `Err` like any other: for `retry_if` to refuse, and sent again after the whole `Retry-After` it carries, or not at all past `BACKOFF_CEILING_MS`; an inner timeout is sent again, as there |
| `retry.retryOnResult(result, attempt)` | `retry_ok_if = F`, `F: Fn(&T) -> bool` | a key of any method declared `idempotent`, a handler included, where the reference keeps it off its entrypoints; the retries spent, the last `Ok` answers |
| `idempotent`, else `GET`/`HEAD`/`OPTIONS` and GraphQL queries | `idempotent`, always written; `ResilienceMiddleware` reads RFC 9110 §9.2.2's methods, `QUERY` and `Idempotency-Key` | never inferred for a method |
| a retry re-runs the pipes, the inner interceptors and the handler | a retry re-runs the method's body, each attempt in a savepoint of the unit's transaction | the unit is not replayed |
| `@Timeout(d)` per attempt | `TIMEOUT_MS`, `timeout = "2s"` | every policy has one; the unit's deadline ends an attempt sooner; one an enclosing policy's timeout cuts counts as a timeout in its own breaker, as there |
| `@Signal()`, `AttemptContext.signal`, `ResilienceContext.signal` | `cancel = token` in a decorated body; the closure's argument in `run` | the attempt's future is dropped at its bound, so the token is for work handed to another task |
| `AttemptContext.attempt`, `ResilienceContext.attempt` | — | not handed: the 2026-11-21 refusal stands; a closure counts its own calls |
| `circuitBreaker.slidingWindow` `count` | `BREAKER_WINDOW` | — |
| `circuitBreaker.slidingWindow` `time` of `A` | `BREAKER_AGE_SECS` of `A` over a `BREAKER_WINDOW` of twice `minimumCalls` or more | an outcome is forgotten at its own age, not by tenths of the window; whole seconds in the environment |
| `failureRateThreshold` `T`, `minimumCalls` `M` | `BREAKER_FAILURES` of `⌈T × M / 100⌉`, the fewest failures the reference opens on | a count, read when a failure is counted: it opens at that many failures whatever the answers between them |
| `openDuration` | `BREAKER_OPEN_SECS` | whole seconds in the environment, an hour at most |
| `halfOpenMaxCalls` (together, judged by their rate) | `BREAKER_PROBES` (in a row, one at a time) | one failed probe opens again |
| `recordIf(error)` | `record_if = F`, `F: Fn(&E) -> bool` | an `Err` it refuses is counted in neither direction, and a probe hands its turn on, as there; the reference never records an inner policy's rejection, where it is here an `Err` like any other, for `record_if` to refuse; an inner timeout is recorded, as there |
| `breaker.state`, `breaker.stats` | `breaker_state()`, `breaker_stats()` (`outcomes`, `failures`, `failure_rate()` from 0 to 1) | the age is applied at the read |
| `breaker.trip()`, `breaker.reset()` | `open_breaker(duration)`, `close_breaker()` | an opening says how long |
| `bulkhead.maxConcurrent`, `maxQueue`, `queueTimeout` | `CONCURRENCY`, `CONCURRENCY_WAITERS`, `CONCURRENCY_WAIT_MS` | the slot is held for the whole run, its retries included; a call waits for one only where `CONCURRENCY_WAIT_MS` is set — none by default, sixty seconds at most — and `CONCURRENCY_WAITERS` bounds how many wait, `0` for no bound, where the reference's `maxQueue` of `0` is no queue and its wait has no limit by default |
| `bulkhead.active`, `bulkhead.queued`; a rejection's `reason`, `active`, `queued` | `concurrency_running()`, `concurrency_waiting()`; `Full` | a call is in one of the two reads at any instant; the refusal says neither which cause nor a count, its `debug` line's `waiters` and the reads do |
| `outboundRateLimit.limit`, `interval`, `maxWait` (token bucket) | `RATE_LIMIT`, `RATE_WINDOW_SECS`, `RATE_WAIT_MS` (token bucket) | counts attempts, a retry included; a token is waited for only when it comes before the unit's deadline; one given back goes to the next in line |
| `limiter.available` | `rate_available()` | — |
| `@Fallback(method \| fn, { handleIf })` | `fallback = method`, `fallback_if = F` | a method of `Self` alone, its signature checked: handed the method's arguments, then the error by value; default: every error (the reference's skips a client error, as its retry does); the failed attempt's savepoint is rolled back first |
| `preset.fallback` for `execute()` | the `Result` a run answers: `.or_else(…)` | written at each call, its filter a `match`; no line, event or counter says a fallback answered |
| `ResilienceEvents.events$`, `nestjs:resilience:<type>` channels | `ResiliencePolicy::events()`, beside the lines on `nest_rs::resilience` and the counters | per policy; bounded, a subscription behind is told how many it missed; a send never runs a subscriber; a line, an event and a counter each say their own fact (below) |
| `retry`, `timeout`, `circuit-open` / `-half-open` / `-closed`, `circuit-rejected`, `bulkhead-rejected`, `rate-limited`, `fallback` | `ResilienceEvent`'s `Retry`, `Timeout`, `Transition`, `Refusal`, `Fallback` | one transition and one refusal event, each saying which; the error as its chain in text, never the value |
| `ResilienceService.circuitBreaker(name)`, `bulkhead(name)`, `circuitBreakers()`, `bulkheads()`, for an app to export | each policy injected by its type; its reads observed on the meter under `opentelemetry` | none is looked up by a name or enumerated; the export is built |
| `ResilienceModuleOptions.defaults`, merged under every preset and decorator | `ResilienceConfig`'s defaults; `for_root` pins a base per instance, the environment overlays it | no defaults shared across policies |
| `name` on a breaker, a bulkhead or a limiter, and its name in a preset: a stage shared across handlers and presets; unnamed, a decorator's breaker or bulkhead is its handler's | the policy: one breaker, one bulkhead and one bucket per instance, shared by every method naming it | a stage is neither shared by two policies nor held by one method; a quota two dependencies share is a policy of its own, its `run` around each call |
| `ExecuteOptions.signal`, the caller's own cancellation | the caller drops the run's future | an attempt in flight is dropped with it and counts in neither direction — but under a unit's deadline, where one dropped past its bound is a timeout; a slot, a token or a probe's turn it held is given back |
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
  and have its refusal told, before the transition that caused it. The phase is
  entered before it is told, and a probe holds its turn before its half-opening
  is told, so what panics out of a telling leaves the breaker in the phase it
  entered and no turn held by a probe that is gone; a telling reached while its
  thread unwinds is caught, where a second panic would abort the process.
- **The framework's own cut counts, a caller's drop does not.** An attempt in
  flight when the timeout of a policy around it gives up the call it runs in
  has timed out in its own breaker, as the reference records it, so a
  dependency that never answers opens the breaker naming it however short its
  caller's timeout. The cut reaches what that call's future holds, as it is
  dropped — a body still read through the middleware included — and no task the
  call spawned, whose attempt goes to its own bound. The inner policy tells no
  `Timeout` and counts no refusal of it: none of its runs is left to answer. A
  body whose read waits across its unit's deadline, as the edge cuts the unit
  there, has timed out the same way: its budget counts from the read, so any
  time its reader takes of its own puts the body's timer past that cut, and
  each hung transfer counted as its head did, a success.
- **A refusal is counted and told as its attempt ends**, before its savepoint
  settles: at the unit's deadline the edge drops the run while the rollback is
  in flight, and the timeout was told by nothing. A run whose savepoint then
  cannot settle answers `Unsettled`, its timeout told and counted all the same.
- **A told error never tells a byte of a userinfo, nor a word a dependency
  wrote in a status.** The chain a line or an event carries is cut wherever
  `://` is written to the scheme, the host and a numeric port, in one pass
  whatever the text; where an `@` follows the authority's end the host cannot
  be told from a userinfo holding the delimiter unencoded, and the scheme alone
  is told. The first cutter read a run of closing brackets once per bracket, a
  dependency's own text stalling the worker, and told a username as a host. A
  reqwest error is said by `nest-rs-reqwest`'s `SafeLink` — its kind, a status
  by its class and its code, and beneath it only what a type that crate reads
  tells, a constant opening it knows or what another crate's `SafeLink` claims
  — since reqwest's own sentence quotes the reason phrase the dependency
  answered, and its causes a trailer hyper refuses, HTTP/2's debug data and a
  certificate's names. rustls's failure is one of those types, read by an
  allowlist of the variants holding nothing the peer wrote but a code of the
  protocol: an alert says what a refused client certificate needs, and the
  OAuth client's and the JWK Set's lines had said it in the published release. This crate claims
  its own refusals there, so a body the policy gave up says its timeout.
- **A `SafeLink` reads its error behind a pointer**: `std` makes an `Arc` and a
  `Box` of an error an error displaying the one held, so an error naming a
  driver's as its `#[source]` behind one handed the line the driver's own
  sentence, values included. `error_message` reads through an `Arc<dyn Error>`
  for every sayer, and each sayer of the workspace reads its type through
  `link_as`, an `Arc` or a `Box` of it included — reqwest's, sea-orm's, sqlx's,
  object_store's, rmcp's, h2's, and core's own of serde's and tokio's: a
  security fix ships at every member reaching the data. A line reads so; what
  an edge answers does not: `DecodeError::in_chain` reads a link that is a
  decode failure, as the published release does, and one an error shares behind a pointer is
  answered opaquely and its line filed, where reading through it told a client
  its payload was at fault and filed nothing.
- **A call its unit had no time to send is no attempt**: it files no line, in
  any configuration, and is told as a timeout of none.
- **No attempt outlives a health indicator's check either**: the probe cuts
  each check at the sooner of `INDICATOR_TIMEOUT_MS` and its own deadline, and
  that cut is the `Deadline` the check runs under, as every edge's is. Dropped
  there instead, an attempt counted nothing, and a dependency that accepts and
  never answers never opened the breaker a readiness indicator runs under.
- **The list's first entry is the field, whatever its name**, one of a key that
  takes a value included. `idempotent`, the one key written alone, is told from
  a field of that name by the key after it, and a field named as a setting of
  the policy's is renamed: the refusal of each says both readings.
- **The attempt's type is read where the signature is**: the body is kept whole
  in a block of its own inside the attempt's, so an item it declares under a
  name of its answer — a `use … as Error`, a `type` — shadows nothing the
  expansion names. It keeps the braces the developer wrote, in parentheses of
  the expansion's own, so what its tail leaves drops by the edition of the
  crate writing it; a label in their place either keeps the expansion's edition
  or is a name the body can break to.
- **A decorated method drops its arguments as the undecorated one does**, last
  declared first, however its run ends — but those its body consumes under
  `record_if`, a limit below. The attempt of a method run once owns
  them, and a closure's captures drop in the order its body first names them
  (`.claude/upstream/rustc.md`): the attempt opens on a statement naming those
  the body names, last declared first. A guard argument was released before the
  writer it guards had flushed. Under an instrument written above it the method
  keeps the order that instrument gives it with no policy
  (`.claude/upstream/tracing-attributes.md`).
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
  refused. A retry's line and event are told once it is certain — as it is
  decided, or once its attempt's savepoint rolled back — and its counter
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
- **A class-level form, a preset merged field by field, and a stage named apart
  from its policy**: a policy is one named instance, a method names it, and its
  breaker, its bulkhead and its bucket are its own.
- **A fallback that is a closure or a free function**: it would have nowhere
  to take the method's arguments from, and no signature to check.
- **A label on a call run by hand** (`source`): a method wanting its name in
  an event is decorated.
- **A burst setting and a bound on the attempts waiting for a token**
  (`RATE_BURST`, `RATE_WAITERS`): no caller, and each a new name.

**Limits, named:**

- **Two decorated calls joined in one unit's transaction take its turn in
  order**: an attempt's savepoint holds the unit until it settles, its call to
  the dependency included, so the other waits inside its own bound and can be
  given up having sent nothing — a timeout counted against a dependency that
  answered, opening its breaker for every caller. A handler sends such calls
  one after the other; closing it is the driver telling the policy an attempt
  waits for its own unit. Pinned as it stands by
  `two_calls_joined_in_one_unit_take_its_turn_in_order_and_the_one_waiting_is_charged_the_wait`.
- **A method calling another under the same policy** keeps its breaker from
  closing once it opened: its own probe refuses the inner call, whose refusal
  fails the probe and opens it again. A method calls another under another
  policy, or the body both share.
- **An error is `'static`**: one borrowing an argument, or generic without that
  bound, is rustc's lifetime error at the answer, naming neither the policy nor
  the remedy — an error owning what it says. Its chain is read as one
  (`&(dyn Error + 'static)`), and no attribute words a region error.
- **What the body of a method run once does not name drops after the
  attempt** — an argument it reads nowhere, the part of a pattern left unbound:
  it stays in the method's frame. One it names under a leading `_` drops with
  the attempt after the others, a read of it being
  `clippy::used_underscore_binding`'s. Names are read by tokens, so a word of an
  argument's spelling reads as that argument, which can cost an
  `unused variable` warning. Closing it is the parameters rebound inside the
  attempt, a signature other than the one written, and an ident an attribute
  can mark as its own.
- **Under `record_if`, an argument the body consumes drops with the attempt**
  that ended before handing it on — cut, failed sooner or refused unsent —
  ahead of those the body only reads, and in the order the body first names
  them. The attempt lends its arguments there, and the statement that sets a
  run-once attempt's order would borrow each whole across the attempt's waits,
  refusing methods that compile: a non-`Sync` argument read in part or named
  unread, a reborrow of a `&mut` answered, a field moved beside one the
  predicate reads. A body opening on `let _ = (&out, &held);` sets the order
  itself. Closing it is the attempt owning every argument the predicate does
  not name, another lending rule than the key's.
- **`E0507` under a key lending the arguments says "cannot move out of value"**,
  the argument underlined: rustc names the variable only where the closure's
  head is the developer's, and there `closure_returning_async_block` fires at a
  closure they did not write, so a crate forbidding it compiled no such method.
- **A fallback at its unit's deadline answers only where its attempt's savepoint
  settles at once**: the rollback comes first, and once it is a round trip the
  edge cuts the unit before the fallback is asked. Closing it is the fallback
  writing above the failed attempt's writes.
- **A policy's breaker times a dependency out at its tightest caller's bound**:
  a caller at two seconds over a policy at five counts a three-second answer a
  failure there, as the reference does.
- **A probe whose run is kept and never polled again holds the half-open
  turn**: only its own timer or its drop gives the turn back, and both need
  its future polled or dropped. A stream that stops being read while it awaits
  a decorated call does that, and every other call is refused meanwhile.
  Closing it is the breaker giving an overdue probe up itself, its late answer
  voided by an epoch.
- **The umbrella's `full` takes `resilience`**, as it takes every capability: an
  app on it declaring a `#[config]` of its own in that namespace fails its
  boot, and one with an attribute of its own named `resilient` on an operation
  has it read as the decorator. The changelog says both. Closing it is `full`
  leaving the capability out until the next major, the owner's to decide.
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
- **A URL holding a space, a control character, a double quote or an angle
  bracket unencoded is read as ending there**, so a userinfo holding one is
  told as a host before it and left as prose after it. A cutter reading prose
  cannot close it; the error's writer does, by encoding its URL or quoting
  none. An address written with no `://` is left as written.
- **A cause `nest-rs-reqwest` cannot read is not said**: hyper's reasons but
  one, HTTP/2's codes, a redirect policy's and a serializer's error are lost to
  every line, an operator reading the kind alone, but for an HTTP/2 GOAWAY in a
  program linking `nest-rs-http`, whose `SafeLink` says it. Saying them safely
  takes `hyper` and `h2` as dependencies of that crate, and a reader of each
  type. What it reads of rustls is an allowlist by outer variant, and of
  hyper-util's tunnel, whose type is private, the text of constant sentences:
  both are read again when either crate moves (`.claude/upstream/rustls.md`,
  `.claude/upstream/hyper-util.md`).
- **A pointer to a pointer is not read** (`Arc<Box<reqwest::Error>>`), nor a
  reference handed out as the link: each is said as `std` displays it, a type
  per depth having no end. A package's own `SafeLink` reads its pointers
  itself, `link_as` being tier 2.
- **An error handing reqwest's sentence on as its own** (`#[error(transparent)]`)
  hides it from the chain, so no `SafeLink` reads it: on the policy's lines its
  URL is cut to its origin and its status to its code, its causes are said as
  they write themselves, and an edge's own failure line cuts nothing of it.
  Closing it is `nest_rs_core::error_message`'s; an error naming reqwest's as
  its `#[source]` is read.
- **An argument a fallback is handed is read**, so one named with a leading `_`
  is `clippy::used_underscore_binding`'s at the key: the name is dropped of its
  underscore. Reading it as the expansion's own would lose a name a
  `macro_rules!` writes apart from the attribute.
- **A call refused beside a probe whose response its caller still holds is
  told that probe's whole bound**, constant for as long as it is held: no
  timer gives a held answer up, and the turn comes back when the response is
  read or dropped.
- **The boot's check of `BREAKER_FAILURES` under an age and a rate bounds the
  policy's own attempts**: a response body held through the middleware counts
  when it ends, tied to no token, outside that bound.
- **A predicate closure whose body answers no `bool`** is rustc's `E0308` at
  its body: what a body answers is the type checker's to read. A predicate
  named `self.method` is its `E0615`, a field holding a function being a
  valid one.
- **Where rustc's own help writes in a key a value the decorator refuses** —
  type arguments after a fallback's name (`E0283`), a call in the field's
  place (`E0615`), a function of `Self` taking no `self` (`E0599`) — the
  refusal it leads to says the remedy.
- **On a trait's method with a default body** the decorator is answered by
  rustc's `E0609`: it reads the method alone and cannot tell it sits in a
  trait.
- **`#[tracing::instrument]` above `#[resilient]` is read by the names its
  expansion writes** (`.claude/upstream/tracing-attributes.md`): one it cannot
  read whole is refused saying which order compiles, by any name beginning as
  the expansion's do; a release dropping that beginning from every name would
  be read as no instrument.
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

## 2026-11-26 — an operation keeps every attribute on its method, every driver owes the attempt's savepoint, and core reads a pointer for every sayer

Taken under the owner's directive of 2026-11-25 (`semver.md`): compatibility
with a published release is no constraint, and the release holding a break is a
major. Three designs the published release had held back went in one change, so the
release this capability ships in is a major, which the entries above name by no
number (`semver.md`, 2026-11-26).

- **`#[operations]` leaves every attribute it does not consume on the
  developer's method**, as every other impl half does: a decorator under any
  name and any `#[cfg_attr]`, an instrument reading `self`, a lint level holding
  the body, an app's own attribute named `resilient`. No impl half reads
  `#[resilient]` by its name any more: the field mark, the `#[cfg_attr]` splitter and the
  `resilience` features of `nest-rs-graphql` and `nest-rs-graphql-macros` went,
  and the umbrella's `resilience` is `dep:nest-rs-resilience` alone. The field
  the macro generates carries what describes it to async-graphql — `#[graphql(…)]`,
  the docs, each `#[cfg]`, one inside a `#[cfg_attr]` unfolded, and each
  `#[allow]`, an `#[expect]` as an `#[allow]` — so an instrument on an operation
  spans the method's body, after the guards, the gate and the pipes, and its
  `err` records the method's own error, never the opaque GraphQL one.
- **Rust's `#[deprecated]` on an operation is refused**, naming
  `#[graphql(deprecation = "…")]`, as `#[routes]` refuses it on a handler
  (`route-deprecation.md`, this date). A `#[graphql(…)]` inside a `#[cfg_attr]`
  is refused too: the derive reading it expands before any condition is
  evaluated, so nothing would read it.
- **A field answering a `Result` under another name whose error async-graphql
  cannot answer is told so first**, decorated or not, in one sentence at its
  return type, rustc's unmet bounds following. The sentence leads because its
  probe is an async method of a trait impl per block, which rustc checks before
  the derive's impls: rustc's order, not a guarantee, which the snapshot shows
  the day a toolchain moves it. An answer holding an `impl Trait`, or `!`, is
  not asked, no turbofish naming it.
- **A gated `#[subscription]` answers a stream a type names or a boxed one**,
  decorated or not, as an ungated one does.
- **Every data driver owes the savepoint an attempt runs in.**
  `nest_rs_database::Executor` says what a handle holds (`Holds::Pool`,
  `Holds::Transaction`), and `HeldTransaction::savepoint` has no default, so each
  attempt of an idempotent call, or of a method naming a `fallback`, runs in a
  savepoint on every driver's transaction — a unit's, a block's, a block of its
  own on a pool unit — and on the pool runs as with no policy. As this line first
  built it, `Executor::savepoint` defaulted to none, and a driver leaving it out
  committed a failed attempt's writes beside its retry's, silently. Held by
  rustc and by an idempotent replay leaving one write on every handle, on
  Postgres.
- **`Problem` is the one vocabulary an edge reads in a chain.** A decode failure
  a chain holds — a dependency's unreadable answer among them — is the server's
  on every edge, answered opaquely and filed, and `DecodeError::in_chain` is
  gone: a handler decoding its client's text says so with
  `Problem::from(DecodeError::from(e))`. A `SafeLink` is typed,
  `SafeLink::of(&say)`, and core reads its type bare, behind an `Arc` or a
  `Box`, through an `Arc<dyn Error>` and inside an I/O error holding one, for
  every sayer; two sayers of one type refuse the boot
  (`payload-never-quoted.md`, this date). A certificate rustls refuses is said
  by its constant reason, read by type off rustls's `CertificateError` and
  webpki's error inside its `Other` (`.claude/upstream/rustls.md`).

**Reopened and built.** The 2026-11-22 refusal of "**Every attribute of an
operation kept on the developer's method**: it refuses programs the published release compiled whose
attribute names a parameter the field leaves out —
`#[tracing::instrument(skip(self, ctx))]` beside a `Scoped<T>`, which is no
`Debug` — a break inside a major." Its one reason was the break, which this line holds as a major;
its changelog's upgrading entry says what a program on the published release
changes.

**Superseded:**

- 2026-11-21: "**An impl half keeps `#[resilient]` on the developer's method**,
  read by its name. `#[routes]`, `#[messages]`, `#[tools]` and `#[processor]`
  leave any other decorator there too; `#[operations]` hands an operation's other
  attributes to the field it generates, where the published release placed them." Every impl half
  leaves what it does not consume, and none reads it by name.
- 2026-11-22, *How each holds*: "**`#[operations]` places an operation's
  attributes as the published release did**, on the wrapper it generates", with its reading by the
  path's last segment, under a feature Cargo unifies across a build.
- 2026-11-22, refused: "**`#[resilient]` read by its full path**: the docs import
  it under its short name, as every decorator is." No impl half reads its path:
  the question is gone.
- 2026-11-24: "the limit of a `#[resilient]` inside a `#[cfg_attr]` on an
  operation, which is split out and kept on the method". Nothing is split: the
  `#[cfg_attr]` stays whole on the method.
- 2026-11-24: "**A `SafeLink` reads its error behind a pointer**: … each sayer of
  the workspace reads its type through `link_as` … A line reads so; what an edge
  answers does not: `DecodeError::in_chain` reads a link that is a decode
  failure, as the published release does, and one an error shares behind a pointer is answered
  opaquely and its line filed". Core reads the type once for every sayer, so no
  sayer calls `link_as`, and no edge answers a decode failure a chain holds,
  bare or behind a pointer.
- 2026-11-21, limits: "A GraphQL field writes its `Result` out, since
  `#[operations]` reads a field's answer by its spelling: a decorated field
  answering any other is refused at its return type, in one sentence, as a
  subscription's aliased `Result` is." Now: any field answering a `Result` under
  another name whose error async-graphql cannot answer — a decorated one's
  `ResilienceError<E>` among them — is told so first, in one sentence at its
  return type, rustc's unmet bounds following.

**Limits closed:**

- 2026-11-22: "**An app's own attribute named `resilient` on an operation** is
  read as the framework's in a build where any crate enables the resilience
  capability … Closing it is reading the decorator by its path, which its
  short-name import defeats." Closed otherwise: no impl half reads it, so the
  app's attribute is the app's.
- 2026-11-22: "**An attribute of an operation reading the method's own `self`** —
  `#[tracing::instrument(fields(depot = self.depot.0))]` — does not compile, as
  in the published release … Closing it is keeping the attributes on the method, a break only a major
  can make (ROADMAP § 2)." Closed: this line keeps them there.
- 2026-11-22: "**A `#[resilient]` inside a `#[cfg_attr]` on an operation** goes
  on the field with the other attributes". Closed by the 2026-11-24 split, and
  now with no split.
- 2026-11-24: "**The umbrella's `full` takes `resilience`** … and one with an
  attribute of its own named `resilient` on an operation has it read as the
  decorator. … Closing it is `full` leaving the capability out until the next major, the
  owner's to decide." The attribute half is closed. The `#[config]` half stands
  as a break of this line, told by its changelog, so nothing is left to wait for.
- 2026-11-24: "A package's own `SafeLink` reads its pointers itself, `link_as`
  being tier 2." Closed: core reads a package's type behind every pointer it
  reads for its own. `link_as` and `decode_failure` stay tier 2 for a sayer
  reading the causes beneath its own error, `nest-rs-reqwest`'s the one.

**Limits, named:** a pointer to a pointer, a reference handed out as the link and
an error handing reqwest's sentence on as its own (`#[error(transparent)]`) stay
unread, as the 2026-11-24 limits say. The aliased `Result` sentence leads by
rustc's order of checks, and an answer holding an `impl Trait` or `!` gets rustc's
errors alone.

**Refused:** calling an operation under `allow(deprecated)`, the change's first
design for keeping Rust's `#[deprecated]` on one: the wrapper would silence the
lint, as `route-deprecation.md` refused for a route, and a deprecation meant for a
client would never reach the schema.

## 2026-11-26 — the run compiled once, a token made only where a call takes one, the breaker read off one word, and a refusal naming its instance

The owner chose the core of the redesign four architects proposed and a judge
merged, built in four steps, each green; the rest waits, named under *Not
built*.

- **One run, compiled once.** `Run<'p>` (`src/run.rs`) is one state machine
  polled by hand, with no type parameter, for every policy, instance and call.
  It polls the call through `Calling` (`src/call.rs`), the one `dyn` seam, so a
  decorated method, or a call of `run` or `run_idempotent`, compiles only
  `Running<'p, R, P, J>`: its closure, its future and its answer in their place
  `P`, and its judge `J`. Each decorated method compiled the whole machine
  again — two state machines and an attempt's future over seven type
  parameters, the instance among them, though every type read it only as a
  name. The instance is read at the policy's face alone: `ResiliencePolicy<I>`
  holds an `Arc<Mechanisms>` (`src/mechanisms.rs`) — the timeout, the retries,
  the backoff, the breaker, the bulkhead, the rate and the boot's three checks
  — and reads `I` where it is built, where it names its remedy and at the
  decorator's seam. `Teller` (`src/teller.rs`) says each fact once, its line,
  its counter and its event, and holds the ten messages; the breaker is built
  from its settings and that teller, where it took eight arguments; and the
  observed reads hold the mechanisms through a `Weak`, which ends the import
  cycle between the breaker and the counters. The two waits a call may never
  meet, for a slot and for a token, are boxed: a run meeting neither reserves no
  room for them, and the bulkhead's unnamed wait makes no type parameter of the
  run. The middleware drives the same run, a success left in the breaker for
  the response's body to settle, and its own retry path is gone. An answer the
  run sends again is let go as its retry is decided, before the retry's line
  and event, where it went just after them: before the wait, either way.
- **A token only where the call takes one.** `#[resilient]` expands to one of
  four tier-2 runs, read off the two keys that change its body's shape: `run`,
  `run_cancellable` (`cancel`), `run_idempotent` (`idempotent`) and
  `run_idempotent_cancellable` (both), the answer probe on each. A body is
  handed its attempt's `CancellationToken` only where its `cancel` names it, so
  a method without the key makes none, nor does `ResilienceMiddleware` for a
  request; `run` and `run_idempotent` keep their signature, which hands every
  attempt one. Whether an attempt makes a token is a zero-sized type parameter
  of its call, `Cancellable` or `Uncancelled`, the closure held as written. The
  method's site is a `&'static ResilientMethod` built in a `const` block of the
  expansion: 8 bytes where a 56-byte value was held once in a run, twice in an
  idempotent one, whose `async fn` holds its arguments twice
  (rust-lang/rust#62958), and once more beside a fallback.
- **A breaker closed or open is read off one word, with no lock.** Each holder
  of its lock publishes as it lets it go — `Holding`'s `Drop`, still under it —
  the instant an open time ends (`open_until`, in nanoseconds past the
  breaker's origin), then one word: the epoch in its upper 62 bits, and whether
  the breaker is closed, closed and quiet, or open. Quiet is a window counted
  with no age, full and holding no failure, which one more success leaves as it
  is, `breaker_stats()` included. Off that word, a call is let through on a
  closed word, in the word's epoch; refused on an open word whose time is left,
  read between two equal loads of the word; and its success is not counted
  where the word is quiet, or the word's epoch has passed the attempt's.
  Failures, probes, switches, `breaker_state()`, `breaker_stats()` and every
  telling take the lock as before, and the mutex is reached only through the
  guard that publishes, so no change goes unpublished. Why each read is the one
  the lock would give:
  - every change is made under the mutex and published there, both stores
    `Release`, a store skipped where the value is unchanged;
  - a pass taken off a closed word is one the lock would have given at that
    word's time: an opening or a switch since bumps the epoch, so its outcome
    counts nowhere, as the lock checks (`held_since`); and a thread never loads
    an older word than one it read, so a word of another epoch than the
    attempt's is newer, and its success returns with no lock;
  - the first `Acquire` load of the word synchronizes with its publisher, so
    `open_until` is that publisher's or a later one; one a later holder stored
    in another epoch shows a different second word, but for a reopening by hand
    straight from open, whose `until` is the open time in force; within one
    epoch the breaker re-enters open only by withdrawing or abandoning a probe,
    with an `until` already passed, and leaves an open time not yet passed only
    by a switch, which bumps the epoch, so an `until` still ahead between two
    equal words is that epoch's opening's, and an ABA within an epoch shows only
    a passed one, which falls to the lock;
  - a read missing a concurrent holder's stores is ordered before that holder's
    section, as a call that took the lock first would be.
- **A refusal names its instance, and the error is `Clone` where `E` is.**
  `TimedOut`, `Open`, `Full` and `Limited` carry a public
  `instance: &'static str`, the policy's marker's `NAME`, empty for the default
  instance, read off the refusing run's teller where each refusal is built; a
  named instance's sentence says it,
  ``the circuit breaker of the instance `payments` is open``. A handler calling
  two dependencies filed `the circuit breaker is open: 503 UNAVAILABLE` for
  either, and its edge's failure line now names the one that refused. The
  default instance's sentences, and every reply, are as they were: no `503`,
  code or `Retry-After` names an instance or a mechanism. Clones share a
  refusal's problem and an unsettled savepoint's cause, so a `#[dataloader]`
  method can be decorated: async-graphql bounds a loader's error `Clone` and
  hands each key of a batch a clone of it, and refused the decorated method
  with its own `E0277`. A refusal holds its
  problem behind an `Arc`, `Display` and `Error::source` written by hand so the
  chain's link is still the `Problem` every edge and the queue downcast to:
  `ResilienceError<()>` is 48 bytes, where its parts made 120.

**Measured**, release, with `benches/policy.rs` in the dev container —
aarch64, fourteen cores, criterion's `--quick` medians in ns, each step against
the one before it on one harness, the load between 0.1 and 6. Before is the run
as this change found it, the median of three runs; now, the last step's, of
two to five. The `contended` group, a decorated method on one policy called by
a thread per core at once, a row one call's wall time on one thread, came with
the change and was first measured on the run already compiled once.

| Row | Before | Now |
|---|---|---|
| `run/bare` | 80 | 62 |
| `run/breaker` | 92 | 68 |
| `resilient/bare` | 79 | 40 |
| `resilient_idempotent/bare` | 100 | 41 |
| `resilient_idempotent/breaker` | 115 | 46 |
| `judged/breaker/ok_under_fallback` | 125 | 49 |
| `judged/bare/err_by_default` | 333 | 285 |
| `refused/run/no_subscription` | 62 | 50 |
| `refused/resilient/subscription` | 134 | 116 |
| `contended/bare` | 111–114 | 70 |
| `contended/breaker` | 4,825–4,894 | 78 |
| `contended/refused` | 2,490–2,493 | 87 |

A decorated method answering at once allocates nothing in a release build,
where it allocated its token; one naming `cancel` allocates that token, and a
call of `run` or `run_idempotent` the one its closure is handed. A refusal
costs its problem's `Arc`, 7 ns through `run` and 2 to 3 through a decorated
method, and the smaller error made a call answering 5 to 10 ns faster on the
decorated, fallback and contended rows. A future answering at once, its body
trivial, measured once the token step was built: `run` 808 → 384 B,
`run_idempotent` 784 → 368, a decorated method run once 824 → 400 and
idempotent 888 → 416, under a fallback 896 → 416 and 960 → 432, and one
naming its token 416 and 432. The bench binary, holding the same decorated
methods and runs, lost 8.1 % of its `.text` (3,420,496 → 3,141,776 B) with
the run compiled once; the code and the compile time per decorated method were
measured on the merged plan's prototype alone, never on the build, and no
figure of it is cited here: no committed bench reproduces it.

**Held by** the suites as they stood, each hygiene snapshot that moved read
before it was blessed, and what the change pins. Red under its mutation: an
idempotent method's last savepoint settled by its verdict, an attempt given up
under a fallback rolled back before it, a token's wait no part of a body's
budget, a retried attempt's timeout naming its method, a call's future
panicking as it is dropped unwinding with no failure counted, an answer a
predicate panicked on let go before its savepoint rolls back, and each place a
refusal is built naming its instance. Pinned beside them: the allocations
above, the breaker answering while another thread holds its lock, a refusal's
instance on the HTTP edge's line and through the middleware, and a decorated
batch's refusal answering each field it served. The breaker's word is
stressed by ten threads failing near half their calls beside one switching it
6,000 times, each bounded at ten seconds of wall clock
(`threads_failing_beside_a_switch_leave_the_window_within_its_bounds_and_exact`).

**The correspondence gains a row**, after `mapErrors`:

| The reference | Here | Differs by |
|---|---|---|
| `ResilienceError.policy`, the name of the policy instance that raised it: a breaker's, a preset's, a handler's (`OrdersController.list`) | `instance` on `TimedOut`, `Open`, `Full` and `Limited`, its marker's `NAME`, and the refusal's sentence (``the circuit breaker of the instance `payments` is open``) | the default instance names none, and no method is named: a decorated method's `host` and `method` are on its lines and events; no reply names it, the `503` being the same whichever policy refused, as the reference's reply names none |

**Names.** `instance` joins `timeout` and `retry_after` among what a refusal
carries, read off the framework's one word for a policy's name —
`Instance::NAME`, the `instance` of every line on `nest_rs::resilience`,
`nestrs.resilience.instance`, `ConfigNotResolvedError::instance` — and never
`policy`, which names the `ResiliencePolicy` here. The tier-2 runs read `run`,
then `_idempotent` where that key is written, then `_cancellable` where
`cancel` names a token.

**Refused:**

- **A wrapper per token shape** (`Cancellable<F>`): with the closure one type
  deeper, rustc reported a body that is not `Send` through every type of the
  run, where it says `` future returned by `ship` is not `Send` ``
  (`resilient_record_if_shares_an_argument`). The marker keeps that snapshot as
  it was.
- **One entry for the four runs**: a closure built against no expected `Fn`
  bound gets no kind and no parameter types, so a body reading its token would
  need an annotation — by reasoning, not measured.
- **`run` reading its closure's arity**, `run(|| …)` beside `run(|cancel| …)`:
  a closure calling a method of its token is then `E0282`, type annotations
  needed.
- **The instance inline beside the problem**: `ResilienceError<()>` became 128
  bytes, where clippy's `result_large_err`, on by default, fires at every
  method answering it — 260 warnings in this crate's suite, one per decorated
  method in an app. A one-word name behind an accessor measured 128 too: the
  three variants holding a duration are equal, which leaves the tag no niche.
- **`Option<&'static str>`**: `ConfigNotResolvedError::instance` already says
  an instance as a `&'static str`, empty for the default one.
- **An instance on `ResilienceRefusal`**, the event's: a subscription is its
  own policy's.
- From the merged plan, judged on its prototypes, which are not committed: the
  run kept generic over the call with the instance alone erased (no smaller
  and no faster — what costs is the call's types monomorphizing the whole
  run); a run driven by boxed `async fn`s (a box per backoff, per savepoint
  settled and per wait for a slot, and a larger future); a call's state as
  plain data given back by hand (a pass or a slot then held by discipline, not
  by a guard's type); dropping the 16 KiB boxing of a call sent once (a buffer
  moved into its closure overflows a 2 MiB worker sooner, the limit the
  footprint tests pin); a file per
  type, which would end the two import cycles left — `teller` and `breaker`
  through `BreakerPhase`, `resilient` and `policy` through `ResilientMethod` —
  at 119 more lines of imports and no case removed; and a sharded or batched
  window, a lock-free GCRA and a packed-word bulkhead: a sharded window opens
  early or late against "opens at that many failures", one atomic GCRA needs a
  128-bit compare-and-swap or loses exactness, and the bulkhead and the rate
  contend only for a dependency answering in nanoseconds.

**Superseded:**

- 2026-11-21: "`idempotent` runs the body through `run_idempotent`, else `run`,
  once", and the answer probe "on `run` and `run_idempotent` themselves": four
  runs, read off `idempotent` and `cancel`, the probe on each.
- 2026-11-22, *How each holds*: "the one `async fn` left, a decorated
  idempotent run's" is two, one per token shape; "So a call answering at once
  allocates nothing but its attempt's token" reads "but the token its call
  takes"; and "what a run waits on besides its call — its attempt's bound, and
  only when they come a slot, the rate's reset, a backoff — shares one place"
  reads: what a run waits on besides its call shares one place — its attempt's
  bound and a backoff in place, a slot of the bulkhead and the rate's token,
  the waits a call may never meet, each in a box allocated when the call meets
  it.
- 2026-11-22, *Limits, named*: the figures of "**A run answering at once costs
  more than the first policy's**", read on 2026-11-24, are this entry's; how a
  run compares with the first policy's `async fn`s stays unmeasured.

**Limit changed:** 2026-11-22, "**Each attempt allocates its
`CancellationToken`**, which its call is handed". Now **a call through `run`
or `run_idempotent` allocates its attempt's `CancellationToken`**, which its
closure is handed whether it reads it or not, tokio-util's token being shared
through an `Arc`; a decorated method makes one only where its `cancel` names
it, and `ResilienceMiddleware` none. Closing it is a call through `run` saying
it takes none, which its signature, handing one to every call, does not let a
policy know; reading the closure's arity is refused above.

**Limits, named:**

- **A call waiting for a slot of the bulkhead or a token of the rate allocates
  that wait once**, in a box beside its token: the run holds the two waits a
  call may never meet in a box, so a call meeting neither reserves no room for
  them (`a_call_waiting_for_a_slot_or_a_token_allocates_its_wait_once`).
  Closing it is every run reserving the larger wait's room, which a call that
  never waits would carry.
- **A refusal allocates its problem**, an `Arc<Problem>` its clones share:
  inline beside a duration and the instance, the problem took the error to 128
  bytes. Closing it takes a `Problem` small enough to ride inline:
  nest-rs-core's, its `problem_type` and `detail` out of line.
- **Two memory-ordering lines of the breaker's word are held by argument and
  review alone**: the second load of the word in `open_left`, and `open_until`
  stored before the word. Each, mutated alone, passes every test, the stress
  test included; the two-line comment in `open_left` and the argument above
  hold them. Closing it is a model check of the publication — `loom` over
  `Published`, `Holding` and `open_left`, a new dev-dependency — or a hook
  between the two loads for a test alone.
- **A breaker with an age takes its lock for every outcome it counts**, each
  success forgotten at its own age; a closed breaker whose window holds a
  failure, or is not yet full, takes it for every success; failures, probes and
  switches always take it. The bulkhead, tokio's `Semaphore`, and the rate
  contend as before, for a dependency answering in nanoseconds.
- **The decorator's integration folder is split by subject**
  (`tests/integration/resilient/`, the attribute's own expansion in
  `expansion.rs`), and mirrors the macro crate's files, as `testing.md` asks,
  in `instrumented.rs` alone: a deviation the one file it was split from
  already made, which the split shows. Closing it is the macro crate's split
  below, or `testing.md` naming this folder as split by subject: the owner's
  pick.

**Not built**, the owner having chosen the core: a refused method's refusals
standing in its body's place; `#[tracing::instrument]` read in one order; an
`impl Trait` inside an answer inferred; `record_if` lending the arguments only
where its predicate names one; the macro crate split by what each file does,
its suite mirroring it; a failure's chain put into words only when a line, an
event or the breaker's opening tells it; the breaker as one transition over
three phases, a probe long overdue giving its turn up; and the four refusals as
one public set, `ResilienceError::Refused`. The limits each would close stand
as the entries above name them, and a failing call still has its chain said for
the breaker's opening whether or not anything tells it: 285 ns on
`judged/bare/err_by_default`.

## 2026-11-26 — a refusal no client reads, proven for every refusal, probe and phase

What the 2026-11-24 entry's units were said to do was run on an open breaker
and a timeout alone; the rest of the family is now run, each test failing under
a mutation of the behaviour it holds:

- **A full bulkhead and a reached rate limit** are refused on a tick, a
  listener, an indicator and a hook as the open breaker and the timeout are:
  the same `Err` path, filed by each edge with the refusal's chain, told to a
  subscription with the method that made it, the unit after it run
  (`tests/integration/resilient/`, `Refusing`'s four members).
- **Every probe**: a refused `#[liveness]` indicator answers `503` on
  `/health/live`, so an orchestrator restarts the process for its
  dependency's refusal, and a refused `#[startup]` one holds `/health/startup`
  down; the other two probes stay up, the chain in the `warn` alone.
- **Every hook phase**: a refused `#[on_application_bootstrap]` hook fails the
  boot naming it, as an `#[on_module_init]` one does; a refused
  `#[before_application_shutdown]` or `#[on_application_shutdown]` hook is
  filed at `error`, and the hook after it runs.
- **Against the live services**: a decorated listener failed, or given up at
  its timeout, after its `Repo` write leaves nothing committed, and an
  idempotent one commits its retry's row alone (`nest-rs-seaorm`'s e2e
  `events.rs`); a job its policy refuses is held back in Redis's `due` for the
  breaker's `Retry-After` by Redis's clock and acknowledged once it runs, one
  refused on its last attempt is a dead letter whose reason is the refusal's
  chain, and one answering on its policy's retry is acknowledged within its one
  delivery (`nest-rs-redis`'s e2e `queue/consumer.rs`).

None of it found a defect. The Redis proofs ran on the dev container's one
server; Sentinel and Cluster take the same settle script, run by hand with
their environments.

## 2026-11-26 — a call nested in its own policy's attempt is that attempt's, an answer that came in time counts, the instruments follow the meter seated, and the breaker's word is model-checked

Four defects, each reproduced by a test that failed first, and one limit closed:

- **A call made inside an attempt of its own policy is that attempt's.** A
  decorated method calling another of its policy — directly, or through a call
  under another — met its own probe's turn, slot and token: the inner call was
  refused by the probe around it, the probe failed on that refusal, and the
  breaker opened again for good; a bulkhead of one refused it `Full`, and a rate
  of one `Limited`; and a failure counted twice. A run now reads, as it opens,
  the policies whose attempt its thread is polling (`src/attempting.rs`, eight
  in place and any deeper behind them, so a call allocates nothing to say it):
  one of its own is let through by nothing again, bound by the attempt it is
  made in, sent once and counted once, by that attempt. Correctness decided it:
  the dependency is called once, so the policy is applied once. A call spawned
  onto a task of its own is a call of its own. The 2026-11-24 limit "**A method
  calling another under the same policy** keeps its breaker from closing" is
  closed, and the test pinning two timeouts for a nested call of one policy
  now pins one. Superseded for every nested call but the first by the entry
  "a call nested in its own policy's attempt takes that attempt's turn first".
- **An attempt answered in time, dropped past its bound, counts as its
  answer.** Under a unit's deadline, an attempt dropped at or past the instant
  its bound ends at counted as a timeout, though its call had answered within
  it and its run had not been polled since — the edge's timer polled first, a
  worker busy. The run is now dropped with its call read once more
  (`Run::dropped`, from `Running`'s drop and the middleware's attempt): an
  answer that came counts as its run would have judged it, one still to come as
  a timeout. Only that drop reads the call: one its caller drops before the
  bound counts for nothing, and one another policy's timeout cuts has timed out.
- **The instruments follow the meter provider `OpenTelemetry::init()`
  seats.** A policy whose first run came before it never reported its observed
  reads, and a counter first recorded before it stayed on the meter that
  records nothing. `nest-rs-core` counts each provider the observability stack
  seats (`__private::meter_provider_seated`, called by `nest-rs-opentelemetry`
  as it seats one); a run compares that count with the one its reads were
  registered at, and a counter keeps one instrument per provider seated, in a
  chain of `OnceLock`s read with no lock. A run pays two relaxed loads, and a
  record one more. A provider installed by hand, with no `init`, is met by a
  counter's first record and a policy's first run alone, as before. The
  2026-11-24 limit "**A policy whose first run precedes
  `OpenTelemetry::init()`**" is closed.
- **The breaker's word is checked under loom.** Its publication is a
  `Board` generic over its 64-bit cell, std's in the process and loom's in a
  model (`breaker.rs`, `published::tests`): every interleaving of holders
  opening, closing and opening again beside a call reading with no lock, and
  every value the orderings let a load read. A call meeting the breaker open is
  told its word's opening's wait or the one published right after it, never an
  earlier one's: each of the two lines named by the 2026-11-26 limit, relaxed
  alone, fails it. loom is the standard for the job, tokio's own, released
  2026-11-08. Its model needs no `--cfg loom`, so it runs in every `just test`;
  a `--cfg loom` build would switch every dependency reading that cfg into its
  own model. The limit "**Two memory-ordering lines of the breaker's word are
  held by argument and review alone**" is closed.
- **An answer sent again is let go before the wait for its retry** already
  held, through the run and through the middleware
  (`an_answer_sent_again_is_let_go_before_the_wait_for_its_retry`): no change.

## 2026-11-26 — a failure put in words only when something tells it, the breaker as one transition over three phases, and one refusal set

Three of the steps the 2026-11-26 entry left *Not built*, on the run as it
stands (one `Run<'p>`, the `Mechanisms`, the `Teller`, the published word):

- **A failure is put in words only when something tells it.** `Verdict` is
  `Copy` and holds no words; what an `Err` failed on stays the call's answer,
  read through `Calling::failure` — and `Reads::failure` for a transport,
  `ResilienceMiddleware` saying a status by its code — as a `Failure`: a
  sentence of the policy's own, or the error. The breaker says it only for the
  failure that opens it: under its lock it reads whether one more failure
  would open it (`Held::failing`, what `count` would answer), and if so lets
  the lock go, says the words, takes the lock again and starts over in the
  attempt's epoch — no `Display` runs under the lock, and two threads failing
  at the threshold open it once. A retry says its error when it is decided,
  only where `Teller::hears` — a subscription open, or a `debug` line on
  `nest_rs::resilience` enabled — and the two given-up lines say those same
  words. Performance decided it: a failing call nothing hears formats and
  allocates nothing. Words that panic count as a panic of the call's, the
  failure counted once, and unwind out of the run after its savepoint rolls
  back; where nothing says them, the error is its caller's `Err`. A
  subscription opened between a retry's decision and its line hears no
  `Retry` of it.
- **The breaker is one transition over three phases.** `Phase` is `Closed`,
  `Open { until }` and `HalfOpen { passed, sent, probe }`: `sent: false` reads
  as open to the lines, `probe: None` is the turn free, and `probe` says when
  the probe out has answered, `By { until, bound }` or `Held(bound)`. Every end
  of a pass is one `settle(epoch, probe, Outcome)` over `Success`, `Failure`,
  `Neither`, `Abandoned` and `Withdrawn`, one `match` of phase and outcome
  (`Breaker::moved`), where seven locked methods each moved one case;
  `probe_out` covers a probe postponed, sent and held. Elegance decided it: one
  place per transition, the tests' reference model green trace by trace. An
  abandoned probe is one dropped once sent: one withdrawn before it was sent
  never files the breaker open.
- **A probe overdue by a whole bound of its own gives its turn up.** In
  `admitting`, a probe whose answer was due a whole bound ago — the bound it
  stored, never the arriving call's, so `refusing` reads it exactly — hands its
  turn to the arriving call, and the epoch moves so its late answer counts
  nowhere. Correctness decided it: a run kept and never polled again, a stream
  that stops being read while it awaits a decorated call, held the turn and
  refused every call for as long as it was kept. The 2026-11-24 limit "**A
  probe whose run is kept and never polled again holds the half-open turn**"
  is closed, and one opens in its place below.
- **One refusal set.** `ResilienceError<E>` is `Failed(E)`,
  `Refused { refusal, instance }` and `Unsettled`; `ResilienceRefusal` —
  `TimedOut { timeout }`, `Open { retry_after }`, `Full`,
  `Limited { retry_after }` — is the set, read off the mechanisms, the next
  member a mechanism added; it refuses anything that sent a call and answered.
  The crate-private `Refusal` is gone: the run, the counters, the teller and
  the event carry the public set, and `ResilienceEvent::Refusal` carries it
  but for a timeout, which `Timeout` tells as the attempt it gave up.
  `ResilienceError::refusal()` and `ResilienceRefusal::retry_after()` read it,
  the error's problem is built from that wait, and the hidden `__problem` and
  the conversion matches that spelled four variants are one each.
  `matches!(error, ResilienceError::Refused { .. })` reads any refusal, where
  the docs' `!matches!(error, ResilienceError::Failed(_))` read `Unsettled`
  too. Elegance decided it; the crate is unreleased, so nothing published
  breaks, and every spelling moved in this change: the suites, the seaorm
  e2e, the docs, the decorator's note, its snapshot.

**Names.** `ResilienceRefusal` was the event's already; it is now the error's
too, one set for one fact. `refusal` is the field and the accessor, as the
event's field was. `Failure` names what an attempt failed on, `Unsaid` the
failure that opens the breaker before its words are said, `Outcome` what an
attempt came to in its breaker; each is crate-private.

**Refused:**

- **The two given-up lines saying their words inside tracing's lazy field**,
  as planned: the words are said once at the decision, for every line and the
  event that hears them, where a lazy field would say them per line and a
  panic of theirs would unwind from inside a line.
- **The simplicity prototype's breaker as written**: built on a pass with no
  borrow and no published word, and its overdue rule read the arriving call's
  bound, which makes `refusing` read a millisecond early. Its phases and its
  one `match` are kept, over this breaker's guards.

**Limits, named:**

- **A runtime stalled past a whole bound voids a live probe**: the breaker
  stays half-open on the new probe, and the late one's answer counts nowhere,
  a success included. Closing it is the probe's own timer firing on time.
- **A subscription opened between a retry's decision and its line** hears no
  `Retry` of it, and a filter enabling `debug` there files none.
- **An error whose `Display` panics** fails its run only where something says
  it; elsewhere it is its caller's `Err` as it came.

**Measured**, release, `benches/policy.rs` in the dev container — aarch64,
fourteen cores — criterion's medians in ns, before on the commit this lane
started from (load 0.6 to 3.3), after on this one, the nesting record and the
seated meter count included (load 1.4 to 6.1, the subset rerun twice):

| Row | Before | After |
|---|---|---|
| `judged/bare/err_by_default` | 306.9 | 40.6 |
| `run/bare` | 62.3 | 62.6 |
| `resilient/bare` | 41.3 | 42.6 |
| `resilient/breaker` | 46.0 | 50.1 |
| `refused/run/no_subscription` | 51.6 | 55.8, 56.6, 57.4 |
| `contended/refused` | 89.3 | 105.8, 100.4, 120.1 |
| `token/new`, code no step touched | 16.4 | 17.1, 16.6, 17.6 |

A failing call nothing hears costs what an answering one does: its chain was
said for every failure, 266 ns of it. Every other row moved between −1 % and
+9 %, within what `token/new`, which nothing here touches, moved across the
same runs (+1 % to +7 %) on a machine whose load rose fivefold: no gain or
loss is claimed for them. What each step adds on the hot path is counted, not
measured: a run reads one thread-local as it opens and writes it twice per
poll of its call, reads the seated count once, and a refusal's counter reads
it once more.

**Superseded:** 2026-11-26, *Not built*: "a failure's chain put into words
only when a line, an event or the breaker's opening tells it; the breaker as
one transition over three phases, a probe long overdue giving its turn up; and
the four refusals as one public set, `ResilienceError::Refused`" are built
above, and "a failing call still has its chain said for the breaker's opening
whether or not anything tells it: 285 ns on `judged/bare/err_by_default`" no
longer holds. The rest of that list — a refused method's refusals in its body's
place, `#[tracing::instrument]` read in one order, an `impl Trait` inferred,
`record_if` lending only what it names, the macro crate split — is the
decorator lane's.

## 2026-11-26 — a refused method is its refusals, an instrument below the decorator alone, an `impl Trait` inferred, `record_if` lending what it names, and the decorator crate split by job

The decorator half of the redesign, on the run as the runtime lane left it:

- **A refused method is its refusals.** What `#[resilient]` refuses on a
  method is emitted as the signature as written, but for the attribute and an
  `async` its body's own `.await` says it was meant with, then the run's mark
  and one `compile_error!` per refusal in the body's place. An error
  expression answers any type and leaves no body to check, so no error of the
  body's and no lint of an argument it no longer reads joins the refusals.
  Elegance decided it: one form, where five — no body, the stand-in, as written
  beside an `impl Trait`, run once without the keys, the `async fn` meant —
  were chosen by what refused the method, each with a reader of the arguments
  to keep rustc quiet; the runtime's `__private::refused` goes with them. What
  is no method, or one already run under a policy, is kept as written beside
  its refusals, and a second decorator is told alone, its list going with it.
- **`#[tracing::instrument]` goes below the decorator alone.** Written above,
  it has wrapped the body in a future answering the method's type, where an
  attempt answers `Result<T, E>`; the decorator read that wrapper by
  tracing-attributes' private names and re-typed it. That reading goes: the
  method is refused at the instrument, saying to write it below, where its
  span covers the whole run and every release compiles. Correctness decided
  it: an app's own `cargo update` of tracing-attributes could turn a method
  that compiled into a compile error, a dependency on another crate's private
  names no type holds. The `__tracing_` prefix is read for the sentence alone.
  A span per attempt is that of a method the body calls, instrumented itself.
  A second `#[resilient]` is found by its run's mark, an item `fn` of that name
  at any depth, so one an instrument wrapped between the two is still told
  twice.
- **An `impl Trait` inside the answer is inferred.** The expansion writes `_`
  in its place wherever it names the answer, and rustc infers the hidden type
  from the body through the attempt's type, as for the undecorated method: a
  `#[subscription]` answers `Result<impl Stream<Item = T>, ResilienceError<E>>`.
  An answer that is an `impl Trait` whole names neither `T` nor `E`, and stays
  refused. Elegance decided it: a refusal and a limit gone.
- **`record_if` lends the arguments only where its predicate names one**, read
  by tokens as a body is — a macro's arguments and a format string's captures
  included — or where an argument is bound by a macro's pattern. A path or a
  closure over the error alone leaves the one attempt owning them, as with no
  key. Correctness decided it: such a method's arguments now drop as the
  undecorated method's do, and its future asks no `Sync`. A false positive
  only lends, as every `record_if` did; a false negative is rustc's
  use-of-moved-value at the key, never a wrong run, since a closure reaches an
  argument by its name alone.
- **The decorator crate is split by job**: `resilient.rs` the entry,
  `args.rs` what the list gives, `method.rs` what the signature says,
  `expansion.rs` an accepted method's expansion, each with its own tests and
  `testing.rs` what they share; one `skip_value` reads past a key's value
  where two readers wrote it. Its integration folder mirrors it — `args/`,
  `method/`, `expansion/` — and `edges/`, the decorator on every impl half, is
  the one folder no file of the macro crate names. Elegance decided it.

**Names.** `Refused::Method` and `Refused::Item` name what was refused;
`decorated` is the entry's reading of an item, `emitted` what it hands rustc.
The snapshot `resilient_under_an_instrument_it_cannot_read` is
`resilient_below_an_instrument`, named for the order it asks.

**Refused:**

- **Keeping the span per attempt by attribute order** through a documented
  marker: tracing-attributes has none, and the upstream ticket asking one is
  the owner's to file (`.claude/upstream/tracing-attributes.md`). Should it
  land, the order can come back read off that marker.
- **The edge hosts folded into one host per impl half**, estimated at 350
  lines of wiring and not counted: only a cut a mutation run proves duplicate
  may be taken, and none was run.
- **`footprint` and `told` moved to the suite's `call.rs` and `teller.rs`**:
  every method of theirs is decorated, and the move would widen the
  decorator folder's fixtures to the whole suite.

**Limits, named:**

- **A refused body's own errors come once its refusals are fixed**, and
  rust-analyzer loses the body's highlighting while it is refused. A private
  helper only that body called can be rustc's `dead_code` beside the
  refusals, by reasoning, not pinned. A toolchain changing what an error
  expression silences is caught by the snapshots on its bump.
- **A release of tracing-attributes binding none of its names under
  `__tracing_`** leaves an instrument above the decorator rustc's `E0308` at
  the body's answers: the sentence is a courtesy, as the limit it replaces
  said.
- **A body whose hidden type nothing names** — an `impl Trait` answered by
  `Err` alone — is rustc's `E0282` at the body, as in plain Rust; a fallback of
  such a method answers the hidden type by name, two `impl Trait`s being two
  types.
- **A predicate naming an argument** still lends every argument to the one
  attempt: one the body consumes drops with the attempt, ahead of those it
  reads, and a non-`Sync` one makes a future that is not `Send`, in rustc's
  words naming no key.

**Superseded:**

- 2026-11-21: "A refused method is kept in the nearest form its body compiles
  in — run once, or typed against a stand-in (`refused`) that nothing runs
  where the receiver or the field is what was refused — so its body adds no
  error of its own": its refusals stand in its body's place.
- 2026-11-24, *Limits*: "**`#[tracing::instrument]` above `#[resilient]` is
  read by the names its expansion writes**" is closed: nothing is read, the
  order is refused. "Under an instrument written above it the method keeps the
  order that instrument gives it with no policy" no longer holds: no method has
  one. "**Under `record_if`, an argument the body consumes drops with the
  attempt**" and "a method sent once that lends its arguments and takes one
  that is not `Sync`" hold for a `record_if` naming an argument alone.
- 2026-11-26, *Limits*: "**The decorator's integration folder is split by
  subject** … mirrors the macro crate's files … in `instrumented.rs` alone" is
  closed by the split above. *Not built*: "a refused method's refusals in its
  body's place, `#[tracing::instrument]` read in one order, an `impl Trait`
  inferred, `record_if` lending only what it names, the macro crate split" are
  built.

**Measured**, counted by non-blank lines that are no comment, outside
`#[cfg(test)]`: the macro crate's code 1,683 lines before, 1,396 after the four
changes, 1,472 once split (the imports each file names); its unit tests 2,362,
then 1,856. The integration suite counts 607 tests before the split and after.
No compile time or code size per method is claimed: an accepted method
expands as before but under a `record_if` naming no argument, which no bench
row measures.

## 2026-11-26 — what the earlier entries left to settle

Each point an earlier report left open, decided by CLAUDE.md's values, the
value named.

- **`run` and `run_idempotent` take a closure of no argument**, and
  `run_cancellable` and `run_idempotent_cancellable` hand the attempt its
  `CancellationToken` — the four spellings the decorator's tier-2 runs already
  had, read off what the call takes, as `cancel` reads it off a body.
  Performance decided it: 341 hand-written call sites, the suites' and the
  docs', took the token to drop it (`|_|`). `benches/policy.rs`, release, in the
  dev container, against the head before the change: `run/bare` 62.0 →
  38.8 ns, `run_idempotent/bare` 62.7 → 39.0 ns, a call answering at once 1 →
  0 allocations (`a_call_answering_at_once_allocates_nothing_but_the_token_it_takes_in_a_release_build`);
  `resilient/bare` 40.7 → 40.6, `token/new` 16.3 → 16.4, untouched. The
  refusal of reading the closure's arity (2026-11-26) stands: two names, not
  one entry guessing.
- **A tick or a listener refused — any failure answering a `503` — is filed at
  `warn`**, `scheduled job failed; unavailable` and `event listener failed;
  unavailable — dispatch continues with the next listener`: a `503` is the
  server unable to serve the unit in time, which every edge with a client
  already files at `warn` (`edges.md`, item 12), and an open breaker refusing
  is the policy working, filed once more on every occurrence it lasts. The
  breaker's opening is its own `warn`, and the failures that opened it each
  their unit's `error`. Correctness of the level decided it (CLAUDE.md,
  *Observability*: `error` when unexpected). The `503` read is the one an
  exception filter answered, else the one the failure's chain carries
  (`nest_rs_core::__private::is_unavailable`). A job's retry was already a
  `warn`, its dead letter an `error`: unchanged.
- **The default breaker stays ten failures of twenty** (2026-11-20). No value
  overturns it: the reference's default mapped gives five of ten, which opens
  sooner on a dependency failing half its calls and refuses sooner a
  dependency that answers the other half; neither is a security or a
  correctness difference, and no bench measures one better. A deployment
  moving it sets `BREAKER_FAILURES`.
- **`link_as` stays tier 2.** Its hole — a community driver's sayer missing
  its own type behind an `Arc` — is closed by the public contract
  (`SafeLink::of` reads every pointer for every sayer, 2026-11-26), and a
  public reader of a link beside it would be a second way to do one thing
  (elegance). Core's own chain search now reads through it too: an edge finds
  a `Problem` named as a source behind a pointer, and an opaque failure's
  cause behind one stops the search, where a `Problem` beneath it answered the
  client — correctness, and the security of the opaque cause.
- **The umbrella's `full` keeps `resilience`**: "full takes every capability"
  is the rule, and compatibility holds no design back (CLAUDE.md, *How we
  work*); the break is told in the changelog. The 2026-11-24 limit naming it
  the owner's is closed.
- **`FieldAnswer` gets no `Debug`.** An operation's attributes stay on the
  developer's method, so `#[tracing::instrument(ret)]` records the method's
  own answer, never the field's (`an_instruments_ret_records_the_value_the_method_answered`);
  nothing reads the wrapper's.
- **A text redacted against its own chain says a sayer's sentence once**: a
  refusal's sayer goes on from its link's `Display` (`…: 503 UNAVAILABLE`), and
  `Redactions` appended the rest again to a text already holding it — the
  ": 503 UNAVAILABLE" the problem detail once said twice. A spelling of the link
  already followed by the whole sentence is kept. Correctness.
- **rustls says nothing on a line**: the workspace takes rustls and
  tokio-rustls without their `logging` feature, whose records quote what a peer
  sent and which only a binary linking `nest-rs-http`'s clamp held back;
  `prefer-post-quantum`, a default, is kept and named — the hybrid key exchange
  preferred. Security decided both (`.claude/upstream/rustls.md`).
- **One TLS double in `nest-rs-testing`, `Upstream`**: the Redis harness, the
  storage suite's proxy and the Postgres double here each accepted a client
  with the test authority's certificate and dialled the service on the
  system's trust, Postgres's `SSLRequest` on both sides; at the third, one
  type (elegance), each double keeping only what it holds back. The Postgres
  fakes of `nest-rs-seaorm`'s and `nest-rs-testing`'s suites answer a pool
  themselves and front no service: they keep their own.

**Stand, no value overturning them:** a predicate that panics counts as a
failure of its call (fail-closed: the run cannot tell its bug from the
dependency's); a parameter bound by a pattern under `fallback`, and a fallback
named by a method alone (one grammar); `RATE_BURST` and `RATE_WAITERS` not
built, and no attempt number in a body (no feature for a hypothetical user); a
refusal of an inner policy counted as the outer call's `Err` (every `Err` is the
dependency failing); a reqwest cause `nest-rs-reqwest` cannot read left unsaid
(security over diagnosis).

**Not done, named:** the default instance's lines say `instance = ""`, a
sentinel where no field would do; `error_message`'s narrower cut (userinfo,
query, fragment, reqwest's reason phrase in a transparent wrapper) is not
built; Valkey's Sentinel and Cluster suites were not run by hand on the
`Upstream` the Redis harness now dials through — the driver did not change, the
double did, so the standalone run is the one held.

## 2026-11-26 — a call nested in its own policy's attempt takes that attempt's turn first, and every other meets the policy

The entry above ("a call nested in its own policy's attempt is that
attempt's") let every run opened inside an attempt of its own policy through
with nothing — no slot, no token, no turn of the breaker, no bound of its own —
on the ground that "the dependency is called once, so the policy is applied
once". An attempt calls its dependency as often as its body says: twenty pages
read inside one `import()` under `CONCURRENCY=2` and `RATE_LIMIT=2` all went out
at once on one token, and twenty failures counted one. That broke what an
operator sets those bounds for — the partner's quota — so it was refused, by
security and correctness: the bound an operator sets holds whatever shape the
code calling it takes.

Superseded for the slot and the turn, which no longer move, by the entry
"an attempt keeps its slot and its turn, sharing each with one call of its
policy at a time".

- **An attempt lends what it holds to the first call of its policy it makes**
  (`src/attempting.rs`, `Lending` in `src/run.rs`). While a thread polls an
  attempt, its run's slot and its turn — its attempt in the breaker, a probe's
  included, and the token it took — sit in that thread's entry, with no borrow
  (`HeldSlot`, `LentAttempt`); the poll ends, or unwinds, taking back what no
  call took. The first nested run to open takes the slot, the first let
  through takes the turn: its outcome is the attempt's, counted once, and the
  attempt's own answer counts nowhere. Every other nested run opens and is let
  through as any call is: its own slot (waiting, or `Full`), token (waiting, or
  `Limited`), turn (refused `Open` beside a probe), its own timeout, its own
  outcome. So the single nested call keeps what the entry above fixed — a
  bulkhead or a rate of one does not refuse it, a probe making it closes the
  breaker, a failure counts once — and a fan-out meets the bounds as the same
  calls made alone do.
- **A slot lent goes with its call**: dropped there, it frees the bulkhead, so
  a call waiting for one is handed it — a lent slot returned to the attempt
  would sit unused while the fan-out waited behind it, and a bulkhead of one
  would refuse every page but the first. The run takes a slot again before its
  next attempt (`poll_back_off`, then `poll_open`), so every attempt is sent
  holding one.
- **A nested run is sent once**, its retries zeroed: the attempt around it is
  what the policy sends again, and retries multiplied at two levels are the
  storm a policy exists to prevent. Held by an idempotent nested call failing
  under three retries, sent once — the test the review found missing.
- **A nested request's body holds what it took**, as any request's does: the
  middleware's `holds` flag no longer clears for a nested run, and the bound
  it holds is its own, so `Run::hold` sums a finite bound.

Held by run.rs's tests: twenty pages under a bulkhead of two and a rate of two
send two, at most two at once, alone and inside one attempt, each failure
counted; a probe's first nested call probes and the one beside it is refused
`Open`; a run whose attempt lent its slot holds one again at its retry. The
cost is a slot and a breaker attempt moved into a thread-local and back on each
poll of an attempt (a flag alone when the attempt lends nothing), no allocation
for eight nested attempts. Measured in `benches/policy.rs`, A/B on one quiet
container, a call answering at once through `run`: bare 39.7 → 42.4 ns,
breaker 46.6 → 54.3, bulkhead 43.9 → 48.1, rate 56.3 → 62.0. Paid, by the
order of the values: the bounds an operator sets hold, then speed.

**Not done, named:** a call of the policy an attempt makes on a task it spawns
is a call of its own and takes nothing of the attempt — the attempt's slot then
stays held beside it, so under a bulkhead of one such a call waits for its own
until the attempt ends or its wait runs out. The middleware's own retry after a
request whose attempt lent its slot takes one again as a run does, but no test
sends a call of the same policy from inside a request's middleware chain.

## 2026-11-27 — an attempt keeps its slot and its turn, sharing each with one call of its policy at a time

The entry above moved the attempt's slot and turn to the first call of its
policy the attempt made. Each then left with that call, and two defects
followed, both reproduced by a test that failed first.

- The rest of the attempt ran holding no slot. Three `push()` calls, each
  fetching a token through a `#[resilient]` helper of its policy and then
  pushing, ran three at once under `CONCURRENCY=1`. Every caller waiting for
  the bound got in beside them.
- The attempt's own answer counted nowhere. A `charge()` whose token helper
  answered `Ok` and which then failed itself counted one success, so the
  breaker never opened, and a probe so shaped closed it onto a failing
  dependency.

Both come from moving something the attempt still needed. Security and
correctness decided it: the attempt keeps what it holds until it answers, so
`CONCURRENCY` bounds its body whatever that body calls, and its own failure
always counts. This supersedes the entry above for the slot and the turn. The
token is unchanged: the first call let through takes it, and every later call
takes its own.

- **The slot is shared, never moved** (`SharedSlot`, `src/attempting.rs`). The
  run holds its slot for its whole run, retries included, as the 2026-11-20
  entry said. While the thread polls an attempt, that attempt's entry says
  whether its slot is free to share. A call of its policy it makes shares the
  slot while no other call does and gives it back as it ends. A call made while
  the slot is out takes a free slot of its own, or waits, within the
  bulkhead's own wait (`CONCURRENCY_WAIT_MS`), for one or for the attempt's
  to come back, whichever comes first; the call giving the slot back wakes the
  ones waiting for it. With no wait, the default, it is refused `Full` at
  once, as the same call made alone is: a fan-out under a bulkhead of one
  reads its pages one at a time on the attempt's slot only when a wait lets
  it. Waiting for the attempt's slot whatever the wait was refused:
  `CONCURRENCY_WAIT_MS=0` says a call finding no slot is refused at once, and
  correctness keeps one meaning for one setting, nested or not, with no wait
  the operator did not set. This answers the entry above's
  reason for moving the slot, a returned slot sitting unused while a fan-out
  waited, without letting the attempt go on unbounded. The re-entry before a
  retry (`poll_back_off`, then `poll_open`, the middleware's loop too) is
  gone: an attempt never loses its slot.
- **The turn of the breaker is shared, never moved** (`SharedTurn`). A call
  sharing it asks the breaker nothing (a probe's turn included) and counts in
  no window of its own. As it ends it gives the turn back with what it failed
  on, said then, since its error ends with it (`Failure::Told`). The attempt
  counts once, as a failure when its own answer failed or a call sharing its
  turn did, and otherwise as its own answer is judged. A failure counts once
  when it propagates, a method's own failure counts after a call that
  succeeded, and a call failing inside a method that answers `Ok` still fails
  the attempt. Calls sharing the turn one after another all count in it, so a
  half-open breaker lets a method's two helpers through on its probe. A call
  made while the turn is out meets the breaker as any call does: it is refused
  `Open` beside a probe, or counted on its own while the breaker is closed.
  `Sent::lend`, `LentAttempt` and `Breaker::adopt` are gone.
- **What a response holds of a shared slot and turn** goes in its `Held`, so a
  body read inside the attempt keeps the slot and fails the attempt.

Held by run.rs's tests:
- `an_attempt_holds_its_slot_past_a_call_of_its_policy_it_made`: three pushes
  under a bulkhead of one run one at a time (three before), and five pages fanned
  out under it, with a wait, are read one at a time.
- `calls_waiting_for_their_attempts_slot_are_woken_as_it_comes_back`: pages
  fanned out through a join polling only the call its own waker woke, as
  `buffer_unordered` does, each read in turn well inside the bulkhead's wait;
  with the wake dropped they wait it out and are refused `Full`.
- `a_call_made_while_another_shares_its_attempts_slot_is_refused_full_with_no_wait`:
  with no wait, the pages after the first are refused `Full` at once.
- `a_call_refused_unsent_on_a_probes_turn_gives_the_turn_back`: a call
  refused `Limited` on the probe's turn gives it back, and the next call the
  probe makes shares it and is sent, where it was refused `Open`.
- middleware.rs's `a_body_read_inside_an_attempt_of_its_own_policy_gives_its_slot_back_as_it_ends`
  and `a_body_cut_short_inside_an_attempt_of_its_own_policy_fails_that_attempt`:
  two parcels read whole in turn under a bulkhead of one both answer, and a
  body cut short, its error swallowed, opens a breaker of one failure.
- `an_attempt_whose_call_of_its_policy_succeeded_counts_its_own_failure`: the
  charge opens a breaker of one failure (it stayed `Closed` before), and so does
  a probe so shaped, as well as one whose helper failed beneath an `Ok`.
- The earlier tests: twenty pages, the probe and its fan-out, the
  walled-in nested failure counted once, and the retry holding its slot.

The cost is measured in `benches/policy.rs`, A/B against the entry above on one
container, for a call answering at once through `run`:

| Policy | Before (ns) | After (ns) |
|---|---|---|
| bare | 39.7 | 41.8 |
| breaker | 50.9 | 56.9 |
| bulkhead | 45.9 | 52.0 |
| rate | 59.1 | 64.6 |

Two things cost it: the flags the attempt shares, moved into the thread-local
and back on each poll, and a larger run. A failure or a wait for the slot is
boxed and allocated only when it happens. The cost is paid by the order of the
values: the bounds an operator sets hold, then speed.

**Not done, named:**
- A call that outlives the attempt it shares with holds nothing of it once
  that attempt answers. Such a call is a future moved out of the attempt, or a
  response handed out of a `run` whose body is read afterwards. Its slot is
  freed with the attempt's, and its body's failure counts nowhere.
- A call spawned onto a task of its own still takes nothing of the attempt.
  It waits for a slot of its own beside the attempt's, as the entry above
  named.

## 2026-11-27 — the default instance's lines and series name no instance

The entry "what the earlier entries left to settle" named it undone: the
default instance's lines said `instance = ""` and its series
`nestrs.resilience.instance=""`, a sentinel where no field would do. Every
other optional field of these lines (`host`, `method`, `failed`, `manual`) is
left out when it holds nothing, and the observability rule refuses a value
that reads as one when it is none, as `actor_id` is never `""`. So a policy's
lines on `nest_rs::resilience`, its counters and its observed reads carry
`instance` only for a named instance, and the default's carry none —
decided by correctness: a filter on `instance=""` matched an absence spelled
as a value. Redis's lines on `nest_rs::redis`, the family's other member, take
the same rule in the same change.

What keeps `""`: `ResilienceError::Refused`'s `instance` and
`ConfigNotResolvedError::instance`, the values a caller matches, which the
entry naming a refusal's instance kept as a `&'static str` over
`Option<&'static str>`; a value, unlike a field, has no absence to spell.

**How a series leaves its attribute out**: its attributes are written led by
the instance's and recorded as a slice cut past that lead for the default
instance (`InstanceAttribute::led`), so a record allocates nothing. Refused:
a `Vec` per record (an allocation on every count), and a match writing each
record twice (two copies of every attribute list to keep in step).

**Held by** `every_line_names_a_named_instance_and_the_default_s_name_none`
(the teller's ten lines), `every_series_names_a_named_instance_and_the_default_s_name_none`
(the four counters), `the_default_instance_s_reads_name_no_instance` (the five
observed reads), and on Redis the plaintext-open, TLS-refusal and Cluster
refusal line tests.

## 2026-11-27 — a shared turn holds only in its epoch, and a nested call keeps its retries where nothing sends its attempt again

The entry "an attempt keeps its slot and its turn until it answers" let a call
sharing its attempt's turn ask the breaker nothing for as long as that attempt
ran. The turn was given back after each call, so a job looping over a partner
(`sync_all()` calling `push(item)` a thousand times, each failure logged) kept
sending every push to it after the breaker opened beside it, on other callers'
failures or by `open_breaker`, each push waiting out its timeout and counting
nowhere, since the attempt's epoch was stale. `open_breaker`'s rustdoc and the
breaker page said every call is refused unsent.

The entry "a call nested in its own policy's attempt takes that attempt's turn
first" zeroed every nested run's retries, "the attempt around it being what is
sent again". Around a run once, or a run's last attempt, nothing sends it
again: an `idempotent` token fetch inside a run-once `charge()` was never
retried, where the same call made anywhere else was.

- **A shared turn carries the breaker's epoch it was let through in**
  (`SharedTurn::epoch`, `Lent::turn: Option<u64>`). A call taking it reads the
  breaker's published word, one atomic load and no lock
  (`Breaker::current`), and shares the turn only while the epoch is the
  same. Once the breaker opened, was switched by hand or had an overdue
  probe's turn taken, the call gives the turn back unsent and meets the
  breaker as any call does: refused `Open` while it is open, let through on a
  pass of its own, counting its own outcome, once it was closed by hand, or
  taking the probe's turn once the open time has passed. Refusing `Open`
  whatever the breaker now says, as the finding proposed, was refused: after
  `close_breaker` it would refuse a call against a closed breaker. Correctness
  decided it, one meaning for a call met with the breaker as it stands. The
  same check runs again after a wait for the rate's token. A probe's own
  calls keep its turn while its epoch is current, which nothing but a manual
  switch or an overdue probe's turn being taken moves.
- **A nested run's retries are zeroed only inside an attempt that may still
  be sent again**, by its run (`idempotent`, retries left) or by a run of its
  policy around that one. Each attempt a thread polls says so in its entry
  (`Attempting::enter(policy, again, lent)`), and a nested run reads the
  innermost entry of its policy (`Attempting::inside`, now
  `Option<bool>`), so a call three deep inherits a resend two levels up.
  Inside a run once or a last attempt, a nested idempotent call keeps its own
  retries: nothing around it multiplies them, so the storm the zeroing
  prevents cannot happen there. Correctness decided it, a call declared
  idempotent is sent again unless something else sends it. The cost is bounded:
  an outer of `n` retries around an inner of `m` sends the inner at most
  `n + m + 1` times, where it sent `(n + 1)(m + 1)` before the zeroing.

Held by run.rs's tests
`a_call_sharing_its_attempts_turn_is_refused_once_the_breaker_opened_beside_it`
(five pushes, the breaker opened by another caller's failure, by hand, or by
hand on a probe: one sent, four refused `Open`; closed by hand, the next sent
on its own and opening it) and
`an_idempotent_call_nested_in_an_attempt_never_sent_again_is_sent_again`
(three sends inside a run once, `1 + 1 + 3` inside a run sent again twice, and
`1 + 1 + (1 + 1 + 3)` three deep), and the middleware suite's
`a_request_sent_inside_an_attempt_never_sent_again_is_sent_again` (a GET inside
a run once, answered `503` then `200`, sent again). The test pinning one send
inside a run once is gone.

Measured in `benches/policy.rs`, A/B on one container (`run`, a call answering
at once): bare 44.5 → 44.4 ns, breaker 59.3 → 58.0, bulkhead 55.4 → 53.7,
rate 70.6 → 64.2. No cost is measured; the faster figures are within the
container's run-to-run spread and claim nothing.

**Not done, named:** a nested call's error the attempt around it swallows,
answering `Ok`, is not sent again by that attempt either: inside an attempt
that may still be sent again, such a call is sent once even though its attempt
will not be resent. Closing it means deciding a resend only once the attempt
answers, which a nested run cannot know as it opens.
