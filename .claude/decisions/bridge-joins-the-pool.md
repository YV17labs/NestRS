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
