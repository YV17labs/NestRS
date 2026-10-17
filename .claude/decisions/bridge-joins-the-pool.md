# A bridge joins the global guard pool at `/graphql` and `/mcp`

`/graphql` and `/mcp` are `Exempt` at the HTTP edge: their request's guards
run in band, in an operation guard. A registered authz bridge used to replace
the global pool there, so a guard the app listed in `use_guards_global` — an
API key, an IP allow-list — never ran on either endpoint once the bridge was
imported. Every access decision belongs to a guard the app declared
(`CLAUDE.md`, hard "no"); a bridge cannot be allowed to drop one.

Now the pool always runs at the request, and the bridge adds to it. A bridge
names the guards it runs itself (`covers()`, by `TypeId`), the pool leaves those
to it, and it runs **where the pool lists the first guard it covers**, or after
the pool when it covers none. `/graphql` stamps `Posture::InBand`, not the
`#[public]` marker: one endpoint whose operations each declare their posture,
so an authentication guard admits a request with no credential there and still
refuses one that does not verify.

**Refused:**

- **The bridge replacing the pool** (the 6.x shape): the defect above.
- **The pool minus what the bridge covers, then the bridge**: a pooled guard
  reading the principal, listed after `AuthnGuard`, would run before the
  authentication the bridge carries and refuse every caller, the misordered
  chain the global bucket's boot validation exists to keep out. Running the
  bridge at the position of its first covered guard keeps the rest of the pool
  in its order.
- **Running the pool and the bridge in full**: a guard listed in both runs
  twice, against *a layer executes exactly once per request* (`edges.md`).
- **Keeping `Public` on `/graphql`**: every guard admitting on `is_public()`
  admitted every GraphQL request, `#[authorize]` operations included, and a
  forged bearer was served as an anonymous caller.

**The limit kept:** a guard the pool lists between two the bridge covers runs
after the bridge, since the bridge runs its guards in one call (`A`, then
`G`). A pool listing `[AuthnGuard, X, AuthzGuard]` beside a bridge covering
both runs `X` after authorization; neither the demo nor the docs list one there.

The bridges and their slots are interim: the pipeline's endpoint guards
replace them, keeping these tests (`one-execution-pipeline.md`).

**2026-10-17 — `feat/endpoint-guards-and-class-gate`: endpoint guards replace
the bridges.** A module declares `EndpointGuards<V, (G1, …)>` as a provider —
`V` a `CarriedView`, the view of the units the endpoint's request carries,
`GraphqlView` or `McpView`, so no other view compiles there — and the HTTP
transport composes the request stage of every self-mount in one
function: the pool, the mount's own guards, then the declared guards inserted
where the pool lists the first one they name, else after it; each guard once,
by `TypeId`, declarations of several modules in import order. What the
bridges' `covers()` said, the declaration's list says. Being a provider, a
declaration is module-gated by construction, and an app with no global pool
is still authenticated by the feature module it imports; a declaration whose
endpoint the app does not mount fails the boot, since its guards would run
nowhere. The stage is
phase-validated at boot as a route's chain is. The operation guards, the pool
runners and the bridges are gone; `/mcp` stays deny-all with neither pool nor
declaration, and its one opening is `McpOptions::posture = Posture::Public`,
which a declaration beside it refuses at boot rather than drop. The limit
above is kept as it was: the declared guards run as one block, so a pooled
guard listed between two of them runs after the block. The tests above keep
their behaviour under the new names
(`transport_parity::endpoint_guards_run_where_the_pool_lists_the_first_guard_they_name`,
`…::a_guard_in_both_pool_and_endpoint_guards_runs_once`).
