# An HTTP route's posture is a guard or an attribute, never an absence or a type

A route either runs a guard — the app's global pool, or a `#[use_guards]` on
its controller or itself — or says `#[public]`; one that does neither refuses
the boot, one error naming every such route and both remedies. A route a guard
covers needs no posture of its own: the guard decides, as a token endpoint
behind its client-authentication guard does. Where a route declares who may
call it beyond its guards, it does so with `#[authorize(A, E)]`, or
`#[authorize(A, bind = S)]` when it loads its subject by id; the row arrives as
an `Authorized<A, E>` parameter, which carries the value and never the
posture. The owner agreed the break on 2026-10-28, with the identity audit's
other public-API changes.

**Refused:**

- **A compile error for a route with no guard and no posture**: `#[routes]`
  cannot see the app's global pool, so it cannot tell a route the pool covers
  from an open one. The boot can, and refuses there.
- **A mandatory posture on every route**: a guard-only route — authenticated,
  gated on no entity — would have to write `#[public]`, labelling as open what
  is not, the mislabel a posture exists to prevent.
- **Keeping the warn**: the opening was an absence, the one the hard "no" on
  posture forbids, and the convention of declaring guards twice existed only to
  cover it.
- **`Bind<A, S>` kept as a value-only parameter**: a type that loads a row and
  answers `403` on it is still an authorization decision written as a type,
  and a second spelling beside GraphQL's attribute. HTTP takes GraphQL's
  grammar instead, and the GraphQL `bind`/`bind_required` helpers leave the
  API for the expansion's private seam. A `Bind<A, S>` resolver parameter, the
  audit's other proposal, is refused for the same reason.
- **`id_arg` on a route**: the subject's id is the one parameter the route's
  path names, which the path already names; a bound route whose path — its
  controller's included, which `#[controller]` counts for `#[routes]` to
  assert — names other than one parameter is a compile error rather than a
  `500` on its first request, and so is a generated `#[crud]` by-id operation
  under a controller path naming a parameter of its own.
- **An entity written beside `bind` other than the service's**: the class gate
  and the mask read the written entity, the load the service's, so the two
  would decide on different entities. The written one is optional and, when
  written, must be `S::Entity`, on both edges.
- **A hand-written `Authorize<A, E>` anywhere but beside `#[public]`**: there
  it is the public-reads spelling, the opposite posture's plumbing; elsewhere it
  would be a gate no attribute shows. Refused by spelling at compile time and
  by its type at boot, so an aliased import is caught too.
- **`#[crud]` documenting an operation with one string** (`docs(get = "…")`):
  an OpenAPI operation has a summary and a description, and a route overridden
  only to carry them was the redundancy the exemplar taught. `api(<op>(…))`
  takes `#[api]`'s two keys; a resolver refuses it, since a field's description
  is a hand-written operation's doc comment, and so does a controller beside
  the operation it writes by hand, as `paginate` beside a hand-written list:
  nothing generated would read the key.

**Open:** whether the app-wide guard pool should be declared by a module the
test harness boots, replacing `use_guards_global` in `main.rs`, so an e2e suite
stops rebuilding it by hand, is the owner's question; this decision leaves the
pool where `apps.md` puts it.

## 2026-10-28 — a capability has a posture of its own

A route a guard covers still needs no posture, and the boot still refuses one
no guard covers. What changed is the case this decision named as the reason a
posture stays optional: a route gated on a capability rather than an entity no
longer needs a hand-written guard to say so. It declares
`#[authorize(Action, Subject)]` on a `#[subject]` (`subject-no-table-holds.md`),
the class gate every edge runs, so `#[public]` stays an honest opening.

## 2026-10-31 — the pool is the root module's

The open question above is answered by the owner's "Tout faire" of
2026-10-28: the app-wide guard pool is `GlobalGuards<(…)>` in the providers
of the module the app boots, replacing `use_guards_global` in `main.rs`, so
an e2e suite boots the pool it covers routes with. A route the pool covers
still needs no posture of its own; a pool declared by a module the app
imports, where it would cover every route without a line of the app's,
fails the boot.

## 2026-11-04 — WebSockets and a gated MCP endpoint take this grammar

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity
audit's second pass, its item on `#[public]`). The refusal above, "a guard-only
route would have to write `#[public]`, labelling as open what is not", held for
HTTP alone: six messages of the demo, behind an upgrade `AuthnGuard` runs, said
`#[public]`, and `nestrs g ws` wrote it. So **`#[public]` now means one thing on
every edge: a caller with no credential is admitted.** A WS message under its
gateway's guards, the pool or a guard of its own, and an MCP operation under a
`Gated` endpoint the pool or `EndpointGuards` cover, need no posture; one no guard
covers refuses the boot like a route. GraphQL and an endpoint `McpAccess::Public`
opens keep the mandatory posture: they admit a visitor in band, so "covered by a
guard" does not mean authenticated there, and the posture is what refuses one.

**`#[public]` that can open nothing refuses the boot**, on what the boot can read
about the request carrying the unit: a guard declaring
`GuardPhase::Authentication` or `Authorization` runs there, or a `Gated` endpoint
runs no guard and denies every request. On HTTP the first is no contradiction, an
authentication guard reading `#[public]` and admitting the visitor; a request's
guards run before a WS or MCP unit is known, so nothing can read it there.

**Refused:**

- **`#[public]` refused beside any guard**, the audit's reading: a rate limit in
  the pool identifies no one, so a visitor does reach the message, and refusing the
  word would leave that opening no spelling, an absence where the first value asks
  for a visible line. `GuardPhase::Other` is what the boot reads. A hand-written
  authentication guard declaring no phase leaves `#[public]` in place, a mislabel
  that errs closed.
- **A subject-less `#[authorize]`** meaning any caller a principal backs (the
  audit's other option): a second way to say what a covering guard says, on HTTP
  already refused by this entry.
- **A warn for a gateway no guard covers** (the transport's, until now): the opening
  was an absence. A gateway's messages say `#[public]` or refuse the boot, and the
  transport refuses a hand-built `Gated` self-mount running no guard, the same gap.
  A gateway no guard covers and declaring no message has no line that could say
  `#[public]`, its hooks running for any caller, and is refused too.
