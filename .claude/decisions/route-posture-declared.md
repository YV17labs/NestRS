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
- **`id_arg` on a route**: the subject's id is the route's one path parameter,
  which the path already names; a bound route whose path names other than one
  parameter is a compile error rather than a `500` on its first request.
- **A hand-written `Authorize<A, E>` anywhere but beside `#[public]`**: there
  it is the public-reads spelling, the opposite posture's plumbing; elsewhere it
  would be a gate no attribute shows. Refused by spelling at compile time and
  by its type at boot, so an aliased import is caught too.
- **`#[crud]` documenting an operation with one string** (`docs(get = "…")`):
  an OpenAPI operation has a summary and a description, and a route overridden
  only to carry them was the redundancy the exemplar taught. `api(<op>(…))`
  takes `#[api]`'s two keys; a resolver refuses it, since a field's description
  is a hand-written operation's doc comment.

**Open:** whether the app-wide guard pool should be declared by a module the
test harness boots, replacing `use_guards_global` in `main.rs`, so an e2e suite
stops rebuilding it by hand, is the owner's question; this decision leaves the
pool where `apps.md` puts it.
