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
  A gateway running no guard at its upgrade with no `#[public]` message has no line
  saying it opens, whatever guards its messages bind, its hooks and what the server
  emits reaching any caller, and is refused too.
- **A boot refusal of `#[public]` beside an ability guard no request reaches**: the
  canonical pool folds `AuthzGuard` over an operation of an endpoint `McpAccess::Public`
  opens, so the refusal would leave that app no spelling for an open tool, the argument
  above for a rate limit. `AbilityGuard::check` builds the visitor's ability on a
  `#[public]` unit no ability reached, as `check_http` does on a route, so the word
  means one thing and the unit reads `define_visitor`'s rules; a gated unit still fails
  closed.

## 2026-11-04 — the reason holds for interceptors too

"A feature never chooses what covers every route" named the pools while a
module's `#[interceptor]` did exactly that at the handler stage. It now holds
there (the owner's "Tout faire (Recommandé)" of 2026-10-31, the identity audit's
second pass; `one-execution-pipeline.md`, the same date): that stage's app-wide
list is the root's alone. What a module still brings runs at the unit stage,
says so in its source, and decides no posture.

## 2026-11-05 — an opening names what it reads

Decided by the first value under the owner's words of 2026-10-12 ("privilégier la
sécurité … production doit être sécurisé, je te laisse arbitrer") and the owner's
"Tout faire (Recommandé)" of 2026-10-31, the identity audit's second pass, whose item
on `#[public]` covers this public break.

**`#[public(Action, Subject)]` is the line a visitor's read writes, on every edge.**
It takes `#[authorize]`'s arguments: the class gate runs against the ability the
caller has, `define_visitor`'s for a visitor and `define`'s for a valid token, that
ability is installed for the unit's reads, and an entity arms the mask. So the
`Authorize<A, E>` parameter goes: the refusal above that kept it "beside `#[public]`"
is replaced. It stated a posture by a parameter's type, looked unused, and answered
`200 []` when a cleanup removed it. No `FromRequest` serves that type now, so a
leftover is `Authorize<…>` is not a handler parameter, by its type and under any
import, in a sentence naming the decorator. A gate another parameter's type declares
still refuses the boot.

**`bind = Service` is taken wherever `#[authorize]` takes it.** The same parameter, the
same load, the same `404` and `403`: a public detail route writes one line where it
wrote a body's `access` and a `match`, what the identity asks of a cross-cutting
concern, and nothing it reads is wider than `#[authorize]`'s load under the same
ability. The elegance value decided, the first having no objection.

**Refused:**

- **`bind` refused on `#[public(...)]`**: the body's `access` and its `match` are what a
  decorator exists to remove, and a visitor's `403` on a row it may not read says what
  an authenticated caller's says.
- **Keeping `Authorize` out of the public path to refuse its import**: `E0432` names no
  remedy, where the trait's own `on_unimplemented` names the decorator.
- **A boot refusal of `#[public(A, S)]` with no ability guard in its chain**: a guard of
  one's own may attach the ability under any phase, so the boot cannot tell; the class
  gate fails it closed, filed at `error` with the remedy, as it does `#[authorize]`.

## 2026-11-05 — a guard identifying no one covers nothing

Under the same words. This decision's first sentence, "a route either runs a guard … or
says `#[public]`", read a rate limit as a guard, so a pool of them opened every unit with
nothing written, the absence the first value forbids. **A guard covers a unit only if it
identifies or authorizes.** Every edge reads `Guard::phase` through the site's composed
stage (`GuardStage::covers`): `GuardPhase::Authentication` or `Authorization` covers,
`Other` does not, on a route, a message, an operation, a hand-written MCP handler and a
`Gated` self-mount. A guard declaring no phase is `Other`, so its unit refuses the boot
and the sentence names both remedies: it errs closed, as a mislabel does above.

`split_at_covered` keeps its word: it reads which of the pool's guards an endpoint also
lists, by type, and decides no posture.

**Refused:**

- **`#[public]` refused beside any guard**, stands, for the reason given above; what
  goes is the same opening with nothing written.

## 2026-11-05 — the reference roots list no posture pool

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity audit's
second pass, its item on the composition the code runs). `ApiModule`'s
`GlobalGuards<(AuthnGuard, AuthzGuard)>` and `LiveModule`'s `GlobalGuards<(AuthnGuard,)>`
repeated what each host declares: the double declaration this decision's refusal of
the warn already says had no other purpose. What covers each edge with the pools gone:
a route, its controller's `#[use_guards]`; `/graphql`, the `EndpointGuards` of
`AuthzGraphqlModule`, which each GraphQL adapter imports, and a field, its resolver's
`AuthzGuard`, since `AuthnGuard::check` admits every unit; a gateway's upgrade and
messages, its own `#[use_guards]`.

**Two chains were not inert. One keeps what it ran.** The broadest site wins, so the pool
ran `AuthnGuard` and `AuthzGuard` ahead of `AudioController`'s `ThrottlerGuard`, listed
first: an anonymous caller answered `401` and spent no budget. With the pool gone the
written order would run, and past its limit that caller would answer `429`. So the
controller writes the order it ran.

**The other moves, and the pool still goes.** `HealthController`'s probes are `#[public]`
routes, not self-mounts, so both pools ran on them, and `AuthnGuard` fails closed on every
route, `#[public]` included, when it could not evaluate a credential. A probe carrying a
credential during a strategy outage answered a `503` problem and now answers its report;
one carrying a refused credential no longer files its `warn`. A request with no credential,
which is every orchestrator's, answers what it did. What decides a pool is what an anonymous
caller answers on a gated unit, and a probe gates nothing and reads no principal. Decided by
the second value: a probe reports the process, and a credential it never reads should not
change that; the first has no objection. The api's committed document holds the chain, since
a guard back on a probe puts its `503` problem back in the document.

A root's pool stays for what no host binds, an API key or an IP allow-list.

**Refused:**

- **The root pool as the one place**, retiring host declarations and `EndpointGuards`
  (the audit's other proposal): a feature imported by an app with no pool would lose its
  guards (`bridge-joins-the-pool.md`), against the owner's standing "declare on the
  provider".
- **`ThrottlerGuard` first on the audio routes**, which this change would have made true:
  a move of what an anonymous caller answers and of whose budget it spends, decided by a
  cleanup rather than for its own reason.

## 2026-11-05 — a class gate no ability guard reaches refuses the boot

Under the owner's words of 2026-11-05 ("on ne veut pas d'angle mort"), replacing the
refusal above, "a boot refusal of `#[public(A, S)]` with no ability guard in its chain".
Its reason, that a guard of one's own may attach the ability under any phase, no longer
holds: `AmbientAbility` is built by authz alone, so what remains is a guard delegating
to `AbilityGuard`. A gate nothing attaches for answered `500` on every call, on
`#[authorize]` as on `#[public(A, S)]`, found by the first caller.

**Every edge's site composition now refuses it**, naming the site and the guard to bind,
reading the unit's chain, then the chain its request ran: a route, a message under its
upgrade, a GraphQL field or an MCP operation under `/graphql`'s or `/mcp`'s request. A
hidden `Guard::__attaches_ability` is what it reads, since the chain holds `dyn Guard`:
`AbilityGuard` answers `true` and `Arc<T>` forwards. The gate's own fail-closed `500`
stays, for a guard answering `true` that attached nothing and for a unit driven with no
boot behind it.

The reference roots' entry above says a field needs its resolver's `AuthzGuard` "since
`AuthnGuard::check` admits every unit". That guard now refuses a gated unit no principal
reached (`check-http-stays-on-guard.md`, the same date); what puts the `AuthzGuard` there
is that `AuthnGuard` declares no `GuardFor<GraphqlView>`.

**Refused:**

- **A public `Guard` method**: a name in the trait's documentation teaches a third place
  an ability comes from, where the framework has one. A guard delegating to `AbilityGuard`
  therefore has no public spelling: it stops delegating and lists that guard beside it,
  which the guide says in words, and the hidden method stays outside the API.
- **A marker trait**: the chain erases to `dyn Guard`, so nothing could read it there.

What it leaves: an ability guard bound at the unit alone under `#[authorize]`, with none at
the request, boots and answers `401`, its sentence naming the guard to bind at the request:
`AbilityGuard::check` builds on a `#[public]` unit only, and this entry does not make a
gated unit's rules depend on where the guard is listed.

## 2026-11-07 — an ability guard at a gated unit alone refuses the boot

Under the owner's words of 2026-11-07 (take up every subject the open list names). The
2026-11-05 entry left an ability guard bound at the unit alone under `#[authorize]`,
with none at the request, booting and answering `401` on every call; seen on a GraphQL
field, a WS message and an MCP operation, an opened endpoint's under the pool too, each
pinned by the suite as booting. A unit its guards alone gate is the same shape, seen on a
WS message and an MCP operation stating no posture and on a `#[field_resolver]`.

The claim the boot reads is now per stage. `Guard::__attaches_ability` stays what a
guard's `check_http` attaches at a request; a hidden `__attaches_ability_to(posture)`
says what its `check` attaches at a carried unit of that posture, by default the same.
`AbilityGuard` answers it for `Posture::Public` alone, which is what `check` does: a
gated unit reads the ability its request attached, and the rules a gated unit runs under
still never depend on where the guard is listed. A route keeps one chain,
`validate_gate_reached`; every carried unit's site calls
`validate_carried_unit` with its handler's posture and whether a class gate
reads the ability, and a guard at the unit attaching for another posture alone, with
nothing ahead of it or at the request attaching one, is refused with its name, the `401`
every call would answer, and where to bind it, class gate or not. A field resolver reads
what the `/graphql` request attached, never what its root field built: a visitor's ability
built at a `#[public]` root reaches no nested read (the *Public reads* page's limits), so the boot refuses it
rather than widening what a nested field reads.

A guard answering the hidden claim `true` and attaching nothing still boots and meets
the gate's `500` (`authorize.rs`'s `a_route_whose_ability_guard_attached_nothing_…`).
Refused at boot only by a claim the boot could check, and none exists across the crate
boundary: whatever `nest-rs-authz` builds to prove an attach, a guard overriding a
hidden trait item builds too, and overriding one is outside the API
(`visibility-tiers.md`). No guard reaching the API can claim it, since `AmbientAbility`
is built by authz alone, so it is kept as a limit, the gate's fail-closed `500` naming
the remedy.
