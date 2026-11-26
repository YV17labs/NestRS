# Changelog — unreleased

Every line is indexed in [CHANGELOG.md](../CHANGELOG.md).

## [Unreleased]

What is below holds breaks: compatibility with a published release no longer
holds a better design back, and the release holding a break is a major
(`.claude/decisions/semver.md`, 2026-11-25). It is numbered when it is cut, and
[Upgrading to the unreleased changes](https://nestrs.dev/upgrading-unreleased/)
is the order to take its breaks in.

### Added

#### Resilience — a named policy around every call to a dependency

`cargo add nest-rs --features resilience` brings `nest_rs::resilience`: a
`ResiliencePolicy<I>` per dependency an app calls, named by an `#[instance]`
marker and built by `ResilienceModule<I>`, which the module whose provider
injects it imports bare (`<PREFIX>_RESILIENCE__<INSTANCE>__*`, or
`ResilienceModule::for_root((Marker, ResilienceConfig { .. }))` to pin a base).
Every module importing one instance shares its one policy, so one breaker counts
the failures of every caller. ([Resilience](https://nestrs.dev/fundamentals/resilience/),
[Failures and fallbacks](https://nestrs.dev/fundamentals/failures-and-fallbacks/),
[Resilience events and reads](https://nestrs.dev/fundamentals/resilience-events/))

- **`#[resilient]` runs the method making the call under its policy**, with no
  closure to write: `#[resilient(carrier)]` sends the call once,
  `#[resilient(carrier, idempotent)]` again after an `Err` or a timeout. It
  takes nine keys after the field — `idempotent`, `timeout` and `retries` (a
  method pins its own, `retries` beside `idempotent`), `retry_if`,
  `retry_ok_if`, `record_if`, `fallback`, `fallback_if` and `cancel`, each
  described below — and every other setting is the policy's: a key naming one,
  before the field or after it, is refused naming its variable and its field,
  and a value of the wrong shape is one error at the value. The field it names
  is the method's `#[inject]` of the policy, so the boot still refuses a missing
  `ResilienceModule<I>`. The method answers `Result<T, ResilienceError<E>>`,
  read by its type, and its body is written as the undecorated method's,
  answering `Result<T, E>` with every coercion that method makes, sent once or
  again. It goes on any host but a WebSocket connection hook, which answers
  `()` and calls a decorated method instead: a provider, a struct built by
  hand, a native `async fn` in a trait impl, a route, a GraphQL query,
  mutation, field resolver or entity, a socket message, an MCP tool, prompt,
  resource or listing, a queue job, a scheduled tick, a listener, a health
  indicator or a lifecycle hook, each taking every key; on an `#[sse]` route or
  a `#[subscription]` it runs the call opening the stream. On a GraphQL
  operation it stays on the developer's method, as every attribute
  `#[operations]` does not consume does, under any name and any `#[cfg_attr]`,
  and the operation spells its `Result<…>` out. A misuse of the
  decorator is a compile error saying what to write: a predicate that is a
  literal is one error in the key's sentence at the value; one that is a
  constant, a function's path of another arity or a closure typed otherwise is
  one too, its note offering the closure form for a method of `self`
  (`|error| self.is_transient(error)`); and a predicate closure
  taking no argument or several, or written `async`, is refused at the closure.
  The list's first entry is the field, whatever its name, one of a key that takes
  a value included (`#[resilient(fallback)]`): a field named `idempotent` is told
  from the key by the key after it (`#[resilient(idempotent, idempotent)]`) and is
  renamed where the method is not, one named as a setting of the policy's is
  renamed, and a comma missing after the field is refused. The field is read by
  its name, so a tuple struct's position and a call in its
  place are refused saying a named field is read; a `fallback` given type
  arguments or written as a call is refused saying to name the method alone; a
  second `#[resilient]` is refused as written twice, under whatever name it is
  imported by, in a `#[cfg_attr]` or below a `#[tracing::instrument]`; a `fn`
  that is not `async`, a `const fn` included, a free function, and an answer
  holding an `impl Trait`, are refused without rustc's own errors beside; and a
  fallback pattern binding `mut` is one error.
  Some kinds stay rustc's own: a predicate closure whose body answers no `bool`
  (`E0308`, at the body); a name `self` lacks, as the field (`E0609`, a trait's
  default body included) or the fallback (`E0599`, a function of `Self` taking
  no `self` included); a getter named as the field, or a predicate written
  `self.method` (`E0615`); a fallback whose type parameters its signature leaves
  open (`E0283`); a fallback taking other arguments than the method's
  (`E0308` at the key, `E0061` at the fallback's name); and a body consuming an
  argument the attempts lend it (`E0507` under `idempotent`, `E0382` under a
  `fallback`) or consuming or changing one `record_if` reads (`E0505`, `E0502`,
  `E0500`). A non-`Sync` argument the attempts lend is refused where the
  method's future must be `Send`, in rustc's words too: under `idempotent` as
  `E0277`, a note pointing at the key, and under `record_if` or a `fallback`
  alone as a future that is not `Send`, naming no key. An error that is no
  `std::error::Error` is refused saying to give the method one, as a
  `thiserror` derive does, and to hold `anyhow::Error` or a boxed error in a
  type of the app's own that is one. A parameter under a `#[cfg]` is handed to
  the fallback under that condition, the fallback declaring it under the same.
  The `debug` lines of a decorated run name its `host` and `method`. One run,
  compiled once, serves every policy and every call: a decorated method
  compiles only its own call and what its keys read of the answer. A run holds
  the call once: a
  release build holds each attempt's future where the run is, whatever its
  size, and the closure there too, so a call answering at once outside a
  transaction allocates nothing but the cancellation token it takes: a
  decorated method takes one only where its `cancel` names it, and allocates
  nothing without; `run` and `run_idempotent` hand one to every attempt;
  `ResilienceMiddleware` makes none. A call waiting for a slot of the bulkhead
  or a token of the rate allocates that wait once, and a refusal its problem
  once, which the error's clones share. Past tokio's threshold for a spawned future,
  16 KiB, a call sent once boxes its closure with the future it makes, its run
  still reserving the larger of their sizes, and a debug build boxes both
  whatever their size, so a body holding a large buffer
  runs on the worker's stack its undecorated method runs on. A fallback's
  future is held as an attempt's is: in place in a release build, boxed in a
  debug build. clippy's `large_futures` says nothing of a decorated method's own
  `.await`s, whichever of its futures is large; a caller awaiting a method whose
  future is large is still told, at its own `.await`. A run lets its closure and its attempt's timer go as it answers.
  `nest_rs::prelude` brings it beside the `resilience` feature, and `run` and
  `run_idempotent` stay for a call that is not a whole method.
- **Classifying an answer, key by key.** By default every `Err` is the
  dependency failing: counted in the breaker and, under `idempotent`, sent
  again. `retry_if = F` (`Fn(&E) -> bool`) says which `Err` is sent again, an
  `Err` it refuses ending the run, and `retry_ok_if = F` (`Fn(&T) -> bool`)
  which `Ok` is — a dependency answering "not ready yet" as a success — the
  last `Ok` answering once the retries are spent, both beside `idempotent`
  alone; `record_if = F` (`Fn(&E) -> bool`) says which `Err` is a failure in
  the breaker, and nothing of the retry. An `Err` `record_if` refuses is the
  dependency's answer, a declined card or a missing
  record: counted as neither a failure nor a success, it leaves a closed
  breaker's window as it was and a probe's turn to the next call, with no
  line, no transition and no event, a half-open breaker staying half-open
  until an `Ok` or a counted `Err`. A timeout and a
  panic are failures no predicate reads, and a timeout is always sent again by
  an `idempotent` method. A predicate is a function's path or a closure,
  reading `self` and the arguments by reference. A `retry_if`, `retry_ok_if`
  or `record_if` that panics fails its attempt as a panic in the body does; a
  `fallback_if` or a fallback that panics, asked once the run is over, unwinds
  from the method and fails nothing more. A method declaring `idempotent`, a
  `fallback` or a `record_if` lends its arguments to each attempt, so its
  future is `Send` only where each argument it takes by value is `Sync` —
  under `record_if` alone, each its body only reads; one run once under none of
  them owns, in its one attempt, the arguments its body names. Every method drops its
  arguments as the undecorated one does, last declared first, however its run ends, but
  for a method run once. Under none of the three keys, what its body leaves unnamed
  drops after the attempt, and an argument it names under a leading `_` drops with
  the attempt, after the others. Under `record_if` and no `fallback`, an argument the
  body consumes is the attempt's own: one it has not consumed when the attempt ends
  drops with it, ahead of the arguments the body only reads and in the order the body
  first names them, which a body opening on `let _ = (&out, &held);` sets last declared
  first. Beside a `#[tracing::instrument]` they drop in the order its future takes
  them. A body keeps its crate's edition for what its tail expression leaves. An answer
  counts in the breaker only in the phase its attempt was let through in: one sent before
  the breaker opened counts nowhere once it has closed again, by its probes or
  by `close_breaker()`.
- **An inner policy's refusal is an `Err` like any other.** One (`Open`, `Full`,
  `Limited`, or the `TimedOut` of its own bound) carried by the `Err` of an enclosing
  decorated method is counted as a failure, and sent again by an `idempotent` one —
  after the whole `Retry-After` an `Open` or a `Limited` carries, and not at all past
  `BACKOFF_CEILING_MS` — unless its `record_if` and `retry_if` refuse it, with a
  `matches!` on the error's own variant holding the `ResilienceError`. An attempt the enclosing policy's timeout cuts counts
  as a timeout in its own breaker, which so times its dependency out at its tightest
  caller's bound. A method calling another under the same policy
  keeps its breaker from closing once it opened, its own probe refusing the inner
  call: call a method under another policy, or share the body. The method's error
  owns what it says (`'static`), one borrowing an argument being rustc's lifetime
  error at the answer.
- **A fallback answers in place of the error a run ends on.**
  `fallback = method` names an `async fn` of `Self`, handed the decorated
  method's arguments, in order, then the error by value
  (`ResilienceError<E>`), and answering what the method answers; the call's own
  error from its last attempt and the policy's refusals — a timeout, an open
  breaker, a full bulkhead, a reached rate limit — each reach it unless
  `fallback_if = F` (`Fn(&ResilienceError<E>) -> bool`) refuses them, an error
  it refuses being the method's answer, as is an `Err` the fallback answers. It
  runs once the run is over, under no policy — no timeout, no retry, counted
  in no breaker — and on an edge has no time past its unit's deadline: one that waits
  is cut with it; where the attempt given up at that deadline had sent a statement in
  its unit's transaction, its savepoint's rollback comes first and the fallback is never
  asked, so one that answers at once answers for the unit only where no rollback is sent. Every
  attempt lends the arguments it is handed next, so the method binds each to a
  plain name, none `mut`, takes no `&mut`, and clones what an attempt consumes;
  a handler taking an extractor apart in its signature (`Path(id): Path<u32>`)
  names it instead (`id: Path<u32>`) and takes it apart in its body.
- **`cancel = token` hands the body its attempt's `CancellationToken`**, made
  only for a body that names it: a
  local of the body and never a parameter, cancelled when the policy gives the
  attempt up at its timeout or its unit's deadline, so work the body handed to
  another task stops with it; a name one of the method's arguments already
  binds is refused, as is an argument bound by a macro's pattern, whose names
  no attribute reads.
- **A savepoint per attempt.** Inside a unit of work holding a transaction, on
  any data driver, each attempt of a method declaring `idempotent` or a
  `fallback` runs in a savepoint of it: what the body writes is kept only when that attempt answers
  `Ok`, and rolled back otherwise — an `Ok` sent again included — so an
  attempt leaves no write for the next to repeat, nor beside what the method
  answers, nor under what its fallback writes. A statement of the unit issued
  beside the method waits until the savepoint settles; a method's future
  dropped mid-attempt, once its savepoint was answered, leaves what holds that
  savepoint unable to commit — its unit, or, nested in another attempt's savepoint
  or in an atomic block, that savepoint or block alone, the unit going on to commit
  what it wrote beside it — and dropped sooner leaves it as it was; two decorated calls joined in one unit's
  transaction take its turn in order, an attempt's savepoint holding the unit
  until it settles, its call to the dependency included, so the other waits
  inside its own bound and can be given up having sent nothing, a timeout
  counted in the breaker against a dependency that answered, and a handler sends
  such calls one after the other; a savepoint the
  database cannot settle ends the method on `ResilienceError::Unsettled`,
  attempted no further and handed to no fallback. A method run once with no
  fallback opens none, nor does any outside a transaction — no unit of work,
  or one on the pool — where an `Err` leaves what the body wrote, as with no
  policy.

- **Every wait is bounded.** Each attempt is given up past `TIMEOUT_MS`, any
  `CancellationToken` its call took cancelled; a retry waits
  `nest_rs_core::Backoff`'s jittered curve, from `BACKOFF_FIRST_MS` up to
  `BACKOFF_CEILING_MS`; a call waits
  `CONCURRENCY_WAIT_MS` at most for a slot of the bulkhead (`CONCURRENCY`), and
  an attempt `RATE_WAIT_MS` at most for a token of the rate limit. A
  duration set in code is held to its variable's ceiling, and at least to a
  millisecond for a `_MS` setting, as from the variable, or to anything above
  zero for a `_SECS` one, under a second included; the two waits are an
  `Option<Duration>`, `None` for none.
- **A call is attempted again only where the caller declares it idempotent**:
  `run` sends it once, `run_idempotent` again after an `Err` or a timeout, up to
  `RETRIES`, never inferred from a method's name. Every `Err` counts as the
  dependency failing unless the method says otherwise (`record_if`,
  `retry_if`); an answer the dependency gave belongs in the call's `Ok`, or in
  an `Err` its `record_if` refuses.
- **A circuit breaker** counts the latest `BREAKER_WINDOW` attempts, opens once
  `BREAKER_FAILURES` of them failed — ten of twenty by default, so a failure
  rate of 50 % after ten calls is `BREAKER_FAILURES=5` — refuses every call
  unsent for `BREAKER_OPEN_SECS` (whole seconds in the environment, an hour at
  most), then lets one probe through at a time, closing
  once `BREAKER_PROBES` probes in a row succeeded; with `BREAKER_AGE_SECS`, an outcome
  older than the age no longer counts, and a breaker a rate limit leaves no room
  for the policy's own attempts to open within its age is refused at boot (a
  response body held through the middleware counts when it ends, outside that
  bound). A call refused beside a probe is told what is left of the probe's
  bound while its call is sent or waits for its token, a millisecond once that
  bound has passed, and, for a probe whose response the middleware's caller
  still holds, the probe's whole bound, the same for as long as it is held. Each transition is a `warn` on
  `nest_rs::resilience` and, beside the `opentelemetry` feature, one more on the
  counter `nestrs.resilience.breaker.transitions`, by
  `nestrs.resilience.instance` and `nestrs.resilience.breaker.state`. A
  transition is told once the breaker's lock is let go: its line, its count and
  its event, in the order the transitions took effect, each once, one thread
  telling at a time. A tracing layer, a meter or a subscription may read or
  switch the breaker from inside a transition's line; across threads a refusal
  can be told before the transition that caused it, and a subscriber panicking
  out of a transition's line loses that transition's count and event. A
  breaker closed, or open with time left, lets a call through or refuses it
  without taking its lock, so callers on many threads do not queue on it there;
  a success takes the lock only where it changes the window — one not yet full,
  holding a failure, or counted with an age — and a failure, a probe, a switch
  and a read of the breaker always do.
- **A breaker is read and switched by hand**: `ResiliencePolicy::breaker_state()`
  answers what a call arriving now meets, a `BreakerState` — `Closed`,
  `Open { retry_after }` or `HalfOpen` — or `None` for a policy with no breaker;
  `open_breaker(duration)` refuses every call for that long, then probes, and
  `close_breaker()` sends them again. Each forgets the window, files the
  transition's `warn` marked `manual` and counts it marked
  `nestrs.resilience.breaker.manual`, on its own replica; a policy with no
  breaker, or an opening the open times of `breaker_open` leave out, is a
  `BreakerError`, never clamped.
- **An outbound rate limit, a token bucket**: at most `RATE_LIMIT` attempts per
  `RATE_WINDOW_SECS`, a retry included. The bucket holds that many tokens,
  starts full and gains one every `RATE_LIMIT`th of the window; an attempt takes
  a token to be sent. One finding none is not sent: it waits for a token, in
  arrival order, within `RATE_WAIT_MS` and the unit's deadline, or ends the run
  `ResilienceError::Limited`, never attempted again. A bucket left idle sends its
  whole limit at once, then a token at a time: up to twice the limit within one
  window's length, which a dependency counting every request of the last window
  takes as half its limit. A token keeps its instant however late the runtime
  wakes the attempt before it, so two attempts sent after a wait may go out
  closer than a token's interval by that lag. Each replica counts its own
  attempts, so a limit the dependency sets for the whole app is divided by the
  replicas sharing it.
- **A bound on the bulkhead's waiters**: at most `CONCURRENCY_WAITERS` calls wait
  for a slot, and the next is refused `Full` at once; a call finding no free slot
  is refused `Limited` at once when its token, behind the line as it stands,
  lies past `CONCURRENCY_WAIT_MS` plus `RATE_WAIT_MS` from its arrival or past
  its unit's deadline, rather than after its wait. That read is advisory: a
  call may still wait for a slot and be refused by the rate after, and one
  refused at once might have been served by a token an attempt ahead of it
  gave back since. A call waits for a slot within the unit's deadline, never
  past it.
- **A refusal is answered `503`, `UNAVAILABLE`, on every edge**: a timeout, an
  open breaker — with the `Retry-After` it knows — a full bulkhead, and a reached
  rate limit — with the time to the next token — as a `ResilienceError`
  implementing `ToProblem`, whose problem rides its chain, so a queue job failing
  with it waits at least that long before its retry, up to an hour. The error is
  non-exhaustive: `Failed` carries the call's own error, `TimedOut`, `Open`,
  `Full` and `Limited` the refusals, and `Unsettled`, the error of the unit's
  own database on an attempt's savepoint, answers the server's own error,
  opaquely. A refusal names the instance of the policy that made it: each
  refusal carries `instance`, the marker's `Instance::NAME`, empty for the
  default instance, and a named instance's sentence says it —
  `` the call did not answer within the timeout of the instance `carrier` ``,
  ``the circuit breaker of the instance `carrier` is open``,
  ``the bulkhead of the instance `carrier` has no free slot``,
  ``the rate limit of the instance `carrier` is reached`` — so an edge's
  failure line, which files the error's chain, names the dependency that
  refused. The default instance's sentences name none, and no reply names one:
  the `503`, its code and its `Retry-After` say nothing of which policy
  refused. `ResilienceError<E>` is `Clone` where `E` is, its clones
  sharing a refusal's problem and an unsettled savepoint's cause, so a
  `#[dataloader]` method can be decorated: async-graphql hands each key of a
  batch a clone of the loader's error. Where no client reads a `503`, a
  refusal — an open breaker, a timeout, a full bulkhead or a reached rate — is
  that unit's error,
  filed by its edge at every occurrence while it lasts: a tick
  files `scheduled job failed` at `error` and the next occurrence runs, a
  listener fails its own unit, its transaction rolled back, and the other
  listeners are still called, an indicator answers down on its own probe while
  the other two stay up — a decorated liveness indicator has the orchestrator
  restart the process for a dependency's outage, a startup one holds the boot
  unfinished — an init or bootstrap hook fails the boot naming the hook, the
  chain in the error `init` returns and on no log line, and a hook of any of the
  three shutdown phases is filed at `error` while the next runs; a job is held
  back in its backend for the refusal's `Retry-After`, and dead-lettered with
  its chain on its last attempt; a `fallback` is how
  such a unit answers instead.
- **`ResilienceMiddleware`, beside the `reqwest` feature,** runs the app's own
  reqwest client under a policy: it sends again what its method (RFC 9110
  §9.2.2's, and `QUERY`) or an `Idempotency-Key` declares idempotent, its body
  one that can be sent twice, on a transport error, a timeout or a `408`,
  `429`, `500`, `502`, `503` or `504`, honouring a `Retry-After` up to the
  backoff's ceiling, in seconds or as an HTTP-date; an answer sent again is
  dropped unread before the wait for its retry, holding no connection or
  stream through it. The policy covers the
  answer's body, read as it comes and never buffered: until it ends it holds
  the run's slot of the bulkhead, a read that fails is a failure in the
  breaker, and once its reads have waited for the dependency, in all, what the
  head left of the attempt's timeout, the read still waiting is given up as a
  timeout; the time its caller takes between reads counts for nothing. Dropped
  before its end, the answer counts as its head did, but for a read left waiting
  past that rest, across the deadline of the unit it is read in, or as another
  policy's timeout drops its reader: a part the dependency still owes then is a
  timeout. So a response is read or dropped, never kept: one held unread holds
  its slot, and a probe's the half-open breaker's one turn; and a body whose
  reads wait longer in all than the policy's timeout — an event stream idling
  between events, a slow download — is cut there, counted against the
  dependency, so such a client goes without the middleware or under a policy
  of its own. Added before `ReqwestMiddleware`, each attempt is a client span of
  its own carrying `http.request.resend_count`; added after it, it refuses every
  request, unsent, saying so. Each request is one attempt of the rate limit, a
  redirect the client follows sent inside it.
- **No attempt outlives its unit**: each ends at the policy's timeout or the
  unit's deadline, whichever comes first, a health indicator's check included,
  and no retry starts past that deadline. An attempt its caller drops counts in
  neither direction; under a unit's deadline, one dropped past its bound counts
  as a timeout, and one another policy's timeout cuts does in a unit or outside one.
  A call's own error answers as its `ToProblem` says or, where it is none, by the
  `Problem` its chain carries, opaquely where it carries none, and a `Retry-After`
  the error's chain carries holds its retry back.
- **The dependency's error is on the policy's lines**: the method's error is a
  `std::error::Error` owning what it says (`'static`), and its whole chain is the `error` of the line a retry
  after it files and of the `warn` filed when it opens the
  breaker, every URL in it told by its scheme, host and numeric port and by no byte of a
  userinfo, a path, a query or a fragment (the scheme alone where an `@` follows the
  authority's end; a URL holding a space, a control character, a double quote or an
  angle bracket unencoded is read as ending there, so a userinfo holding one is told
  in part; an address with no `://` is left as written), and a status reqwest's own
  sentence says by its code, never the reason phrase after it; under
  `ResilienceMiddleware` a status is said by its code alone, never a header or a body.
- **A policy's state lives in its process**, each replica's breaker, bulkhead
  and rate its own, and the pages say so.
- **A policy tells what it does three ways.** A line on `nest_rs::resilience`
  for an operator: at `debug` a retry (`attempt failed; retrying the call`, or
  `attempt answered; retrying the call` for an `Ok` sent again), a retry given
  up, a wait for a token, a call the bulkhead or the rate refuses, a fallback
  answering and a savepoint the database could not settle, its `error` the
  cause's chain; at `warn` a breaker's
  transition. A call an open breaker refuses, a call its unit had no time to send (no attempt: told once as a
  `Timeout` event with a zero timeout)
  and a run ending on a timeout file no line of their own, in any configuration. A refusal is
  counted and told as its attempt ends, before its savepoint settles: a run whose last
  attempt timed out and whose savepoint then cannot settle answers `Unsettled`, its
  timeout told and counted all the same. Every duration a line files is in whole
  milliseconds, rounded up. An event in code: `ResiliencePolicy::events()`
  opens a subscription to each retry, attempt given up at its bound, call
  refused unsent, transition and fallback as a `ResilienceEvent`, read with
  `next().await` (no `Stream`), bounded at 256 unread events, past which it is
  told how many it missed; with none open, none is built. An event names a
  decorated method's `host` and `method`, and a call run through `run`,
  `run_idempotent` or the middleware by neither. And the state a policy holds
  now: `breaker_state()`, `breaker_stats()`, `concurrency_running()`,
  `concurrency_waiting()` and `rate_available()`, each `None` for a mechanism
  the policy has off. A call counts as running from the moment its own future
  holds its slot, and a waiter handed a freed slot reads as waiting until it
  runs.
- **Beside the `opentelemetry` feature a policy has nine instruments**, by
  `nestrs.resilience.instance`. Four counters: `nestrs.resilience.retries`,
  `nestrs.resilience.refusals` (by `nestrs.resilience.refusal` — `timed_out`,
  `open`, `full` or `limited`), `nestrs.resilience.fallbacks` and the breaker's
  `nestrs.resilience.breaker.transitions`. Five observed up-down counters,
  additive across replicas, registered as the policy is built, each only for a
  mechanism the policy has on: `nestrs.resilience.breaker.status` (by
  `nestrs.resilience.breaker.state`), `nestrs.resilience.breaker.outcomes` (by
  `nestrs.resilience.breaker.outcome`, `failure` or `success`),
  `nestrs.resilience.concurrency.running`, `nestrs.resilience.concurrency.waiting`
  and `nestrs.resilience.rate.available`. A policy whose first run precedes
  `OpenTelemetry::init()` never reports its observed ones, nor a counter it
  first recorded before it. `refusals` counts each run ended on the policy's
  own refusal, whether its caller is answered `503` or a fallback answers in
  its place. A transition to
  `open` not marked `manual` is failures opening the breaker, a probe failing,
  or a probe its caller dropped once sent, no probe of its series having
  succeeded. A count is not always an event's: a retry the
  policy then refuses unsent is told as a retry and counted as the refusal its
  run ends on, and a timeout the run sends its call again after is told and
  counted nowhere.

Thanks to Just-Replicant, whose resilience proposal (#7) brought the bulkhead's
wait bounded by the call's deadline and the counters of retries and refusals,
both carried into this one.

#### A tick answers an error of its own

`#[cron]`, `#[every]` and `#[after]` take a method returning `Result<(), E>` for
any `E` converting into `anyhow::Error` — `anyhow::Result<()>` as before, or an
error of its own such as a `thiserror` type — and, as before, one returning `!`,
whatever its type is called; each is read by its type, so a tick can carry
`#[resilient]`. Any other answer is one compile error saying what to write — for
a box, `anyhow::Error::from_boxed` — and an error borrowing its host is refused
in rustc's own words, `E0716` for an `async` tick and `E0515` for one that is
not.
([Schedule](https://nestrs.dev/schedule/))

#### A unit's deadline is ambient on every edge

`nest_rs::core::Deadline` — the instant a unit ends by — is ambient in every
unit its edge cuts at a deadline: an HTTP request, a WebSocket message, a
GraphQL or MCP operation, a job attempt, a tick, a listener call, a health
indicator's check. Work inside can stop waiting before the cut; a unit nested in
another ends by the sooner.

#### `HttpsPeer`, the server an HTTP client under test calls

`nest_rs::testing::HttpsPeer` (`tls`) is a loopback HTTPS peer presenting a
`TestAuthority` certificate, answering each request from a script of
`PeerAnswer`s — a status and its headers, a whole body (the headers
`PeerAnswer::header` adds ride every answer that writes a head, a body answer
included), a body cut short or
stalled halfway, a body in two parts the suite lets through one at a time, or no
answer at all — and keeping each `RequestHead`.

#### The umbrella's `full` takes `resilience`, and `reqwest-middleware` is public

- **The umbrella's `full` feature takes `resilience`.** **Breaking:** an app on
  `full` declaring a `#[config]` of its own in the `resilience` namespace fails its
  boot, its type and the framework's both claiming it: name another `namespace`,
  or list the features the app uses in place of `full`.
- **`reqwest-middleware` is public in `nest-rs-resilience`**, at the `0.5`
  `nest-rs-reqwest` already holds.

#### A module is imported as its type

`#[module(imports = [...])]` reads a module of a named instance as the type it
is, `RedisModule<Cache>`, spelled as the field injecting what it opens spells
`RedisConnection<Cache>`.

### Changed

- **`RedisModule::<Cache>` in `imports`, the turbofish the published release taught, is refused** at
  its `::`, naming the type to write. **Breaking:** write a module of a named
  instance as its type, `RedisModule<Cache>`.
- **A data driver's handle says what it holds, and nothing it owes has a
  default.** `nest_rs::database::Executor` is one method, `holds()`, answering
  `Holds::Pool` over a `HeldPool` — `transaction`, the block `atomic` opens there,
  and `outbox`, what a cancel reaches — or `Holds::Transaction` over a
  `HeldTransaction` — `after_commit`, `savepoint` and `outbox`, what a push is
  written into. The port reads `atomic`, `after_commit`, a resilience attempt's
  savepoint and a push's outbox off that answer, so a driver leaving one out no
  longer compiles: with the defaults the published release gave it, it dispatched events before its `COMMIT`
  and filed jobs its rollback undid, and a savepoint left to a default would have
  committed a failed attempt's writes beside its retry's. `atomic` answers
  `AtomicError::Unsupported` only outside any unit of work. **Breaking** for a
  data driver implementing `nest_rs::database::Executor`: answer `holds()` —
  `Holds::Pool` over a type implementing `HeldPool` (`transaction`, `outbox`), or
  `Holds::Transaction` over one implementing `HeldTransaction` (`after_commit`,
  which holds the work and hands nothing back, `savepoint`, `outbox`) — every
  method required, the outbox through the new `Outbox` (`write`) and
  `OutboxKept` (`remove`, `remove_unique`) traits, and recover your handle with
  `executor.holds().downcast_ref::<T>()`: `as_any`, `non_transactional`,
  `atomic`, `write_outbox`, `remove_outbox` and `remove_outbox_unique` are gone.
  An app changes nothing: `atomic`, `after_commit`, `Jobs::push` and `cancel`,
  `#[resilient]` and `Repo` keep their signatures.
  ([Writing a database driver](https://nestrs.dev/database/writing-a-driver/))
- **`Problem` is the one vocabulary an edge reads in a handler's error chain.**
  A resolver, gateway handler, tool or interceptor whose error chain holds a serde
  error answers `INTERNAL`, its chain filed at `error`, on every edge, where
  GraphQL, WebSocket and MCP answered `INVALID_ARGUMENT` and filed nothing: a
  partner's answer that did not decode was blamed on the client and hidden from
  the operator, while HTTP answered `500` and filed it. A handler decoding its
  client's own text says so —
  `serde_json::from_str(&text).map_err(|e| Problem::from(DecodeError::from(e)))?`
  answers `400` under `INVALID_ARGUMENT`, where it failed and never the value. A
  `PipeError` carries the `Problem` it
  answers (`PipeError::problem`, and it implements `ToProblem`), so a refusal a
  handler returns, bare or wrapped in its own error, answers `400`
  `INVALID_ARGUMENT` on HTTP too, where it answered `500`, and a line says it as
  `<message>: 400 INVALID_ARGUMENT`; a queue job whose method returns one, bare
  or wrapped, dead-letters at its first attempt as any client error does, where
  it was retried within its budget; and an exception filter whose
  `type Exception = Problem` reads a chain catches a pipe's refusal it found
  there, a per-argument one on GraphQL and MCP and a global data pipe's on
  WebSocket included. An HTTP interceptor failing with the
  framework's `DecodeError` — a `QueueError::Serialize` — answers `500`, where it
  answered `400`. **Breaking:** `DecodeError::in_chain` is gone; a handler
  decoding its client's own text by hand says so as above, and a client reading
  `INVALID_ARGUMENT` for an upstream's unreadable answer reads `INTERNAL`.
- **On MCP, a `Problem` under `INVALID_ARGUMENT` answers JSON-RPC's
  `invalid_params` (`-32602`)**, a `422` included, as a pipe's refusal already
  did; any other `4xx` stays `invalid_request` (`-32600`), a `5xx`
  `internal_error`. **Breaking** for an MCP client reading `-32600` for an
  argument a tool refused: it reads `-32602`.
- **A `SafeLink` names the type it says**: `SafeLink::of(&say)`, over
  `fn(&T) -> Option<String>`, the function submitted by reference. Core reads `T`
  for every sayer — bare, behind an `Arc` or a `Box`, through an `Arc<dyn Error>`,
  and inside an I/O error holding one — so a sayer downcasts nothing, and a
  package's no longer leaves its type said by its `Display` behind a pointer it
  did not read. An I/O error holding an error a `SafeLink` claims
  (`io::Error::other(DbErr)`) is said by that sayer, where the held error's own
  sentence was said. A type has one sayer: two functions claiming one refuse the
  boot, naming the type and both (`ContestedSafeLinkError`, read where the app is
  built, before any module collects), where one of them went quiet; one function
  linked twice is one sayer. **Breaking** for a package or an app submitting a
  `SafeLink`: `SafeLink::new(say)` over `fn(&dyn Error) -> Option<String>` is
  gone. Name the type and submit the function by reference —
  `fn say(error: &AcmeError) -> Option<String>`,
  `nest_rs_core::inventory::submit! { nest_rs_core::SafeLink::of(&say) }` — one
  entry per type, with no downcast and no `link_as` of its own; a sayer over
  `&dyn Error` does not compile (`E0277`).
  An app that submitted its own `SafeLink` over `reqwest::Error` drops it, or
  claims an error type of its own holding reqwest's as its `#[source]`.
  ([Binding a database driver](https://nestrs.dev/database/binding-a-driver/))
- **An attribute on a GraphQL operation applies to the method you wrote**, as on
  a route's handler, no longer to the field `#[operations]` generates. The macro
  takes off a method only what it consumes — its role, its posture, its layers,
  `#[meta]` and async-graphql's `#[graphql(…)]` — and leaves every other
  attribute there, as written and in order, on a query, a mutation, a
  subscription, an entity or a field resolver alike, a parameter keeping every
  one but `#[graphql(…)]`: a decorator under any name and any `#[cfg_attr]`, an
  instrument reading the method's own `self` and an attribute of the app's own
  apply to the method. The field carries what describes it to async-graphql:
  `#[graphql(…)]`, the docs, each `#[cfg]` and each `#[allow]`, an `#[expect]` as
  an `#[allow]` of the same lints. **Breaking:** a `#[tracing::instrument]` that
  named the field's parameters skips the method's own — `skip_all`, or its
  `Scoped<T>`, `Piped<P>` and `&Context` parameters by name; rustc's `E0277`
  names the one with no `Debug` — and spans the method's body, after the guards,
  the gate and the pipes, which the request's `graphql.field` span covers, its
  `err` recording the method's own error. A `deny`, `warn` or `#[cfg_attr]`'d
  lint level on an operation now holds its body. Rust's `#[deprecated]` on an
  operation is refused: write `#[graphql(deprecation = "…")]`, which the schema
  and its introspection show. A `#[graphql(…)]` inside a `#[cfg_attr]`, which no
  macro read, is refused: write it plain, and put the condition on the method as a
  `#[cfg]`. ([Queries and mutations](https://nestrs.dev/graphql/queries-mutations/))
- **A `nest_rs::database::atomic` block dropped before its `SAVEPOINT` was
  answered leaves its unit as it was.** Cancelled (a timeout, a `select!`)
  while its unit waited for a connection, for its `BEGIN` or for the block's
  `SAVEPOINT`, a block left its unit unable to commit, though no
  statement of its own had run; the unit now commits what it wrote beside
  the block. A block dropped before any of these already left its unit as it
  was, and one dropped once its `SAVEPOINT` was answered still leaves
  what it sits in unable to commit: its unit when it sits directly in it, and
  otherwise the block or savepoint it is nested in alone, the unit going on to
  commit what it wrote beside it. An app that relied on such a cancellation to
  refuse its unit's commit answers an error from the unit instead.
- **`nest_rs_reqwest` claims reqwest's errors as a `SafeLink`**, so an error chain
  holding one says it without the URL the request was sent to, whose path and
  query hold what the caller sent: its kind in reqwest's words and, for a status,
  its class and its numeric code (`HTTP status server error (503)`), never the
  reason phrase or a word the dependency wrote. Beneath it is said only what a type
  the crate reads tells, an I/O error's sentence or kind and a decode failure
  without its value, a failure of TLS in rustls's own words where they quote
  nothing the peer wrote (the alert a peer refusing the handshake sent, by its name;
  a port that speaks no TLS; a peer sharing no version or suite), a constant
  sentence for a timeout, a failed lookup, a closed connection, too many redirects,
  a scheme reqwest does not speak and a proxy that opened no tunnel, a certificate
  the client refuses by the constant reason rustls gives
  (`invalid peer certificate: BadSignature`, `: UnknownIssuer`,
  `: CaUsedAsEndEntity`,
  `: certificate does not allow extended key usage for server authentication`,
  `: certificate expired`), never the names, dates, usages or identifiers it
  holds, or a cause another crate's `SafeLink`
  claims, as that crate says it: a response body the policy cut says the policy's own sentence, and an HTTP/2 `GOAWAY` is said
  without its debug data in a program linking `nest-rs-http`; a cause of another
  type is not said, so a line holds none of hyper's other reasons, HTTP/2's other
  codes, a rejected Encrypted Client Hello or rustls's catch-alls. The error is read
  through an `Arc`, a `Box` or an `Arc<dyn Error>` around it, and inside an I/O error
  holding one, one pointer deep. An app's own `SafeLink` of `reqwest::Error` beside
  it refuses the boot: an app wording reqwest's errors its own way claims an error
  type of its own holding one as its source. An error handing reqwest's sentence on as its own
  (`#[error(transparent)]`) holds it as no link: on a resilience policy's lines its URL is cut to its origin and its status to its code, its
  causes said as they write themselves, and on an edge's own line nothing is cut.
  A reply whose detail is such an error's sentence (`ProblemDetails::from_error`)
  reads the same way: without the URL, its causes named.
- **A health indicator's check runs under a `nest_rs::core::Deadline`**, the sooner
  of `<PREFIX>_HEALTH__INDICATOR_TIMEOUT_MS` and
  `<PREFIX>_HEALTH__PROBE_DEADLINE_MS` from the probe's start, or its request's when
  that is sooner. The two cuts and their reasons are unchanged; a check
  that reads `ambient::<Deadline>()` reads that instant, where
  it read none. A call a check makes under a resilience policy ends there, a
  timeout its breaker counts: a decorated indicator then reads `check failed`,
  filing `health indicator failed`.
- **Every page, generator and boot remedy writes a module of a named instance as
  its type**, `RedisModule<Cache>`, where the published release taught `RedisModule::<Cache>`.

### Security

- **Only a route's posture opens its reads.** `#[routes]` probed every
  parameter's type for a `RouteResponseShaper`, and a route taking one, any
  crate's, read under its guards' ability with no posture: two routes under the
  same guards and none declaring one answered `current_ability()` `None` and
  `Some`. The class gate is now `nest_rs_authz::class_gate::<A, S>`, emitted from
  `#[authorize(A, S)]` or `#[public(A, S)]` as on every other edge, and no
  parameter's type is read for it. **Breaking:**
  `nest_rs::http::RouteResponseShaper`, `ResponseShaping`, `RouteFuture`,
  `ShapedEndpoint` and `nest_rs::authz::http::Authorize` are gone; a route that
  read scoped rows through such a parameter declares its posture, and code that
  shaped a response through the trait moves to an interceptor. The boot
  refusals of a gate a parameter's type declared, and of a shaper on an `#[sse]`
  route, go with what they refused.
- **An HTTP reply's `detail` is a sentence written for the client, never an
  error's `Display`.** `HttpError::new(e, 4xx)`, a `ResponseError` writing no
  `as_response` and `ProblemDetails::from_error` sent the error's sentence as
  `detail`, a denylist of wordings its only filter: a driver's
  `10.0.0.3:5432 as svc_ledger`, and a partner's `?token=…` once its
  `reqwest::Error` was wrapped in `#[error(transparent)]`, reached the client.
  Each now answers its status's problem with no `detail`, the error kept as the
  cause; the framework's own refusals (`Json`, `Query`, `Form`, `Multipart`,
  `Path`, a body read, a path parameter) write theirs. WebSocket, MCP and
  GraphQL already took only a sentence passed in. **Breaking:**
  `ProblemDetails::from_error` is gone; a client's sentence is a `Problem`'s
  `with_detail("…")`, a decode of its own text
  `Problem::from(DecodeError::from(e))`, and a `ResponseError` whose `Display`
  was the client's implements `ToProblem`.

### Fixed

- An `#[expect]` on a GraphQL operation, written plain or under a `#[cfg_attr]`, is
  checked on the developer's method alone, the field's generated wrapper carrying
  it as an `#[allow]` of the same lints, under its condition. The wrapper was
  handed the expectation too, and a lint the wrapper does not trip left it
  unfulfilled — a warning, an error where `unfulfilled_lint_expectations` is
  denied.
- A gated `#[subscription]` — `#[authorize(A, S)]` or `#[public(A, S)]` — answers
  a stream a type names or a boxed one (`BoxStream<'static, T>`), decorated or
  not, where it failed with `E0308` at `#[operations]`; its items are filtered by
  the rows they hold, as an `impl Stream`'s are.
- A field answering a `Result` under another name whose error async-graphql cannot
  answer is told so first, in one sentence at its return type, rustc's unmet bounds
  following.
- `nest_rs::core::error_message` reads a driver's error behind an `Arc`, a `Box` or an
  `Arc<dyn Error>` as the error it holds, so a line naming one as its `#[source]` no
  longer quotes a value: the database's, the storage's, an MCP session's, HTTP/2's, a
  decode failure and a joined task's.
- The JWK Set fetcher and the OAuth client read a `Retry-After` in both forms
  RFC 9110 gives it, an HTTP-date counted from the answer's own `Date`, where
  each read only its seconds; the OAuth client no longer takes `+5`, and reads
  a value past what fits as the longest wait rather than none.
