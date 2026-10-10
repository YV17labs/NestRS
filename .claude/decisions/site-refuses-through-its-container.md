# A site composed at `configure` refuses through its container

A route's or a gateway's layer chain is composed as its transport configures,
inside a mount: `Controller::mount` and a self-mount's closure return the route
tree, with no `Result`. A declared layer no imported module provides, or a
route whose handler scope lists an authorization guard before the
authentication guard it reads, had nowhere to fail: the first was dropped
(fail-open), the second answered `500` on every request.

Now such a site files its refusal on the container it composes against
(`refuse_site`, tier 2), mounts what denies — an opaque `500`, a message check
refusing every frame — and whatever configured the transports reads the
refusal before any hook runs and fails the boot with it: `App::run`, and the
test harness's `TestAppBuilder::build` and `HeadlessApp::spawn_transport`.
It is `ContainerBuilder::refuse` for the built container: a `register` has no
`Result` either, and refuses the same way.

A declaration a boot check reads still refuses there, before the mount: a
controller's guards, and a gateway's upgrade guards and event guards, in their
`HttpBootCheck`, so a transport configured by hand fails too. Every other
declaration refuses only at the mount, through this channel: a controller's
pipes, interceptors, filters and exception filters, and the whole handler
scope.

**Refused:**

- **A `Result` out of the mount**: `Controller::mount`, the self-mount closure
  and the transport's configure loop are the HTTP surface the route grammar and
  the pipeline adoption rewrite (HTTP-07, EXEC-14); changing their signatures
  for one check would collide with both. The pipeline's own sites can return
  the refusal directly and leave this channel unused.
- **Failing closed at the route alone**: no request runs without its layers,
  but the deployment boots, and a misordered chain is then found by its users.
  A wiring defect is a boot error (`container.md`).
- **Panicking at the mount**, as the WebSocket table did: a refusal with no
  sentence a reader can act on, and a transport's configure is no place to
  unwind through.
- **Reading the refusal in the transport's own `configure`**: the one place
  every caller passes, but the transport is HTTP's, and a site of any edge may
  refuse; the kernel reads it once for all of them.
