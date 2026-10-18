# The shutdown phases run after every transport stopped

Recorded on 2026-10-18. The three shutdown phases take their names from
NestJS, not their timing, and the migration guide said nothing of it. NestJS
12's `app.close()` runs `onModuleDestroy` and
`beforeApplicationShutdown(signal)` **before** it closes its HTTP server, then
`onApplicationShutdown(signal)` after it. nestrs runs all three once every
transport's `serve` has returned, or was abandoned past its stop bound, after
the drain. A ported module that stops a consumer in
`beforeApplicationShutdown` therefore stops it here up to the longest stop
bound later (20.5 s at the defaults). Correctness decided to keep the contract
and write it down, since every extension encodes it.

**Chosen — the three phases are a staged unwind after serving.**
`OnModuleDestroy` is each module's own teardown, dependents first;
`BeforeApplicationShutdown` is cross-module work once every module tore down
(a lease several of them shared); `OnApplicationShutdown` comes last, and the
resources the modules opened close after it. No unit its transport stopped
within its bound runs through a hook it may depend on — the invariant a
scheduled tick already gets, whose bound is spent before the hooks. A
transport still serving past its bound is abandoned, named at `error`, and the
hooks start anyway: a stop nothing bounds is a `SIGKILL`.

What NestJS code does in the early slot has a home that runs **at the
signal**:

- stop starting new work, or end a loop: `AppLifecycle::stopping()`, a token
  cancelled the instant the app turns `Draining`, joined in the host's own
  shutdown hook;
- know the signal: `AppLifecycle::cause()`, readable in every shutdown hook;
- stop taking requests: the transport's own drain, which stops accepting at
  the signal; taking the replica out of the load balancer before that is the
  platform's (Kubernetes `preStop.sleep`, ECS's deregistration, Nomad's
  `shutdown_delay`), shown in the demo's chart.

**Refused:**

- **Moving `OnModuleDestroy` and `BeforeApplicationShutdown` before the
  drain**, as NestJS orders them. A request, a job or a socket message still
  running would meet the providers those hooks tear down — a pool closed, a
  client released — and fail for a reason that has nothing to do with it.
- **A readiness `503` or an in-framework delay during the drain**
  (`<PREFIX>_HTTP__SHUTDOWN_DELAY_SECS`, Terminus's
  `gracefulShutdownTimeoutMs`). It shortens the window every request has left
  inside the grace, an HTTP/2 connection cannot be told to close per request
  (RFC 9113 §8.2.2 forbids `Connection: close`), and every supported platform
  has a hook that takes a replica out of rotation before its signal
  (Kubernetes `preStop.sleep`, Nomad `shutdown_delay`), or does it itself
  (ECS, Cloud Run). It can return as an additive knob, off by default, if a
  supported platform turns out to lack the hook.
