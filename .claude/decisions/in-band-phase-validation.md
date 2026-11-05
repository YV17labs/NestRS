# The in-band guard chains are not phase-validated

`boot_validate_guards` turns a misordered chain — an authorization-phase guard
ahead of the authentication-phase one whose principal it reads — into a named
boot failure instead of a deployment that denies everything with nothing to say
why. It runs at HTTP's route mounts, over the global bucket, and at a WS
gateway's upgrade (an HTTP `GET`, so it runs `check_http`, the entry the check is
written about).

Per WS message it was shipped and proved wrong in both directions at once. A
per-message chain runs `check_ws_message`, while the validation reads what a
guard produces and expects for `check_http` — and `AuthnGuard` keeps the no-op
`check_ws_message` by design. Applied there it was silently green on a chain
where nothing attaches a principal, and a false boot failure on the split-scope
shape the authorization docs sanction. The ordering half would transfer; the
principal half needs a per-message notion of "produces".

`#[operations]` and `#[tools]` compose their chains on the first dispatch that
reaches a site, after every boot check, and nothing at boot enumerates the
sites. Validating them would need a link-time registry of sites walked at boot.

## 2026-10-23 — the registry exists; walking it for phase order stays Deferred

The link-time registry of sites this entry said was missing now exists. Every
GraphQL field's and every MCP operation's `Site` is a `static` its decorator
declares, listed for the boot (`GraphqlSiteRegistration`, each host's
`operation_sites`), and composed at the kernel's wiring step, so a layer one
declares that no module provides fails the boot. Phase-validating those guard
stages at the same step is additive and Deferred: the ordering half would
transfer as is, while the principal half still needs a notion of what a guard
produces for `check` rather than for `check_http`, the one the validation
reads.

## 2026-11-04 — the principal half, for a guard that requires its principal

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity audit's
second pass). An in-band unit's guard requiring its principal
(`check-http-stays-on-guard.md`) is checked at the wiring step, over the unit's own
chain and the one the request carrying it runs, whose `check_http` is what attaches.
That is the notion of "produces" this entry waited for, and only that half needs it:
ordering and a guard doing without its principal stay Deferred.

## 2026-11-05 — ordering is held in band, before 7.0 publishes

Under the owner's words of 2026-11-05. "Additive" no longer holds: the semver page
makes a new refusal a break, so this one added after 7.0 stops an app that boots, and
every site already reaches `validate_carried_chain` at wiring. `validate`'s loop and
its first-run rule are one function both call, and an in-band chain listing
authentication after authorization refuses the boot. It reads the request's chain
ahead of the unit's, so a pooled guard is where its request ran it, and an
authentication guard at a unit whose request ran an ability guard is refused too: the
ability was built before its principal existed, so every gated call answers `401`. The
closure composes the request only for a chain holding an authentication guard, a
pooled `AuthnGuard` among them, once per site at boot.

**Refused:** reading the unit's chain alone, the narrower fix: it leaves that second
misorder booting and refusing everyone with nothing to say why, this entry's first
sentence.

What it leaves: the phase is its author's word, which no type can check, and a
hand-written MCP `ServerHandler` has no chain to read.

