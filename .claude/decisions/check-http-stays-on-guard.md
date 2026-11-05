# `check_http` stays on `Guard`, and the capability markers stop where they do

Moving `check_http` to an extension trait buys nothing: `nest-rs-guards` depends
on `nest-rs-http` unconditionally, so every build linking the guard core links
the HTTP stack whatever the consumer asked for, and a `cfg` on the method saves
no bytes. (`cargo tree -i poem` on a headless feature set names the crates a
worker actually pays for; each is its own report.)

Two residual gaps are accepted:

- A guard may declare an edge's marker trait without overriding the matching
  `check_*`. That is a deliberate line a reader sees next to the method it
  should have been. Closing it means four `check_*` traits with no defaults,
  and since every execution site holds `Arc<dyn Guard>`, a guard serving three
  edges would need three container registrations.
- `use_guards_global` takes no capability bound. A global guard legitimately
  serves whichever edges it implements; requiring `HttpGuard` would refuse a
  GraphQL-only one, and the alternative is a per-edge global list — four
  declarations where the developer wrote one.

## 2026-10-12 — two entries over a unit, one declaration per view

`check_http` stays a method of `Guard`; the three in-band entries became one
`check`, and the four markers became `GuardFor<V>`, `V` the view of the units a
site dispatches (`HttpView`, `GraphqlView`, `WsView`, `McpView`). Both entries
take the unit's `UnitContext` and admit by default.

The premise above — `nest-rs-guards` depends on `nest-rs-http` — is no longer
true: the guard core depends on `nest-rs-core` alone, and each view lives in
its edge's crate. The reason `check_http` stays on `Guard` is now that a guard
serving several edges is one registration: the pool and every site hold
`Arc<dyn Guard>`, and an extension trait per edge would need one container
entry per edge it checks.

The two residual gaps are unchanged, in the new words: a guard may declare
`GuardFor<V>` without overriding the entry `V`'s units run, a line a reader
sees; and `use_guards_global` takes no `GuardFor` bound, since a global guard
serves whichever edges it implements.

A guard whose one `check` is the same on every edge declares
`impl<V: UnitView> GuardFor<V>` once (`ThrottlerGuard`), so it owes its crate
no feature per edge.

**2026-10-23** — poem left the dependency tree; `cargo tree -i nest-rs-http`
on a headless feature set names the crates a worker pays the HTTP stack for.

## 2026-10-29 — `check_http` runs `check`, and the edge files every denial

Agreed by the owner with the identity audit's breaking changes ("Tout faire",
2026-10-28). `check_http`'s default now runs `check`, so a guard writes its
decision once, and overrides `check_http` only where the request differs from
the unit — it reads the request, answers in its protocol's own words, or a
value missing at the request means something else than at the unit. The first
residual gap is closed for HTTP: a guard declaring `GuardFor<HttpView>` and
writing only `check` refuses with it. The reason `check_http` stays on `Guard`
stands — one registration, the pool and every site holding `Arc<dyn Guard>` —
since the change is a default body, not an extension trait.

The gap stays on the in-band views: a guard declaring `GuardFor<GraphqlView>`
and writing only `check_http` admits every field. Closing it would make
`check` default to `check_http`, which a unit carried inside a request cannot
run: its view is not the request's. The second gap is unchanged, and now
reaches further: `use_guards_global` takes no bound, so a pooled guard writing
only `check` runs it on every request as well — the route, the WS upgrade,
the `/graphql` and `/mcp` requests — failing closed where it reads a view the
request lacks; one whose decision is the unit's alone admits the request in
`check_http`.

`files_its_denials` is gone with it: a guard declares the target its denials
are filed on (`fn target`, a method because an associated `const` would make
`Guard` no longer dyn-compatible) and names why on the `Denial`
(`with_reason`, `with_source`), and every edge files each guard denial once.

Refused: a `#[guard]` attribute emitting the injectable, the `Layer` and the
`GuardFor` impls. It reads as `#[interceptor]`, which a module brings to every
unit it serves, and a module-brought guard would decide posture outside
`#[use_guards]`; `#[derive(Layer)]` drops the empty impl instead.

## 2026-10-30 — `check` has no default: no guard admits a unit it never decided

Agreed by the owner with the identity audit's breaking changes ("Tout faire",
2026-10-28); the gap the 2026-10-29 entry left on the in-band views closes
fail closed, at the type. `check` lost its admitting default, so a guard
declaring `GuardFor<GraphqlView>`, `GuardFor<WsView>` or `GuardFor<McpView>`
and writing only `check_http` no longer compiles, and neither does an empty
`impl Guard for X {}`, which passed everything on every edge. A guard whose
decision is the request's alone — `AuthnGuard`, `ClientAuthenticationGuard` —
admits the units the request carries with one visible `Ok(())` in `check`,
the mirror of a unit-only guard admitting the request in `check_http`.

What the 2026-10-12 entry refused is not this: four `check_*` traits without
defaults would need a container registration per edge. One required method
on `Guard` keeps one registration and `Arc<dyn Guard>` at every site.

Refused: `check` defaulting to a denial. It closes the gap at runtime only,
fails a pooled request guard on every in-band unit until someone reads the
`500`, and holds less than the compiler does. Refused too: detecting at a
site whether `check` was overridden — Rust cannot tell, and a marker the
developer writes would be the same visible line, weaker.

What it costs: an admitting `check` written as an `#[async_trait]` future
allocates once per in-band unit the pool runs it on, where the default
allocated nothing. The framework's two return a zero-sized future instead, so
they still allocate nothing; on HTTP, `check_http` still runs one future.

**2026-10-31 — the pool's spelling.** `use_guards_global` is now
`GlobalGuards<(…)>`, declared by the module the app boots; like the builder
call, it takes no `GuardFor` bound, for the reasons above.

## 2026-11-04 — `require` answers by cause, and the 2026-10-29 carve-out no longer arises

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity audit's
second pass). "A value missing at the request means something else than at the
unit" was the leak: `/graphql` admits a visitor, so every guard reading the
principal had to know its edge, and the exemplar wrote its decision twice.
`cx.require::<T>()` now answers by cause for the principal a guard declares through
`expected_principal`: **the boot refuses a chain nothing in which attaches it** —
a route's, and at the wiring step a unit's with the request carrying it, read off
`produced_principal` — so its absence at a unit is a caller a guard attaching it
admitted without one, the caller's `401` on every edge, never a `500`. A `#[public]`
route that set a rejected credential aside answers the `401` it earned
(`RejectedCredential`, RFC 6750 §3's `error` with it), as an `Ambient` parameter does.
Any other value's absence stays the wiring `500`, and a guard may declare
`impl<V: UnitView> GuardFor<V>` once. The classification runs in the stage's refusal
path, so the hot path pays for nothing.

`PrincipalClaim::or_visitor` names a guard that does without it: `AbilityGuard`
reads an actor where one is and decides a visitor with its factory's visitor branch,
so nothing attaching it stays a `warn`, a controller of `#[public]` routes being a
legitimate shape. An attaching guard that declares no `produced_principal` is refused
beside a guard requiring its principal, and declares it.

Refused: attaching a marker where an authentication guard admits a visitor, for the
requiring guard to read. It would have told a wiring gap from a visitor at request
time, at an allocation and an ambient scope on every anonymous request; the boot
tells them apart for nothing. Refused too: reading the unit's posture to decide, a
GraphQL field's posture being its own, never its request's.

What it leaves: an attaching guard bound at the unit alone, whose `check` attaches
nothing (`AuthnGuard` on a resolver, which `GuardFor` already refuses), passes the
boot and answers `401`, as it would for a visitor.

## 2026-11-05 — a gated unit no principal reached is refused at the unit

Under the owner's words of 2026-11-05 ("on ne veut pas d'angle mort"). The 2026-10-30
entry let a guard whose decision is the request's admit every unit it carries in one
visible `Ok(())`. That held only where the request itself refuses: `/graphql` is
`InBand`, so `AuthnGuard` admits an anonymous request there, and `_service`, which
states no posture and has no class gate, then ran a constant `Ok` and answered the
whole SDL; an `AuthzGuard` beside it found the visitor's ability and passed too. Seen
red, under a pooled `AuthnGuard` and under `EndpointGuards` alike.

**A guard over a `Strategy` refuses a `Gated` unit no principal of its type reached**,
`401` in the class gate's own sentence and reason; `AuthnGuard` and
`ClientAuthenticationGuard` run one function, so neither restates it. A `Public` or
`InBand` unit states its own posture and is admitted as before. **`AbilityGuard`
refuses a visitor's ability on a federation root field**, read off `FieldRole`, the one
unit a visitor's ability reaches with no posture after it. **`_service` runs every
guard its request ran**, the pool and `EndpointGuards`, since the reference composition
lists no pool and no member's posture stands behind its answer. `_entities` keeps the
pool alone: each `#[entity]` states its posture, so what one opens beneath
`EndpointGuards` is a line a reviewer reads.

What it costs: one ambient read on each gated unit a pool runs this guard on. The
admitting future stays zero-sized, so it still allocates nothing.

**Refused:**

- **A refusal in the guard stage, reading `produced_principal`**: the ambient context
  has no read by `TypeId`, and the stage would decide for a hand-written guard whose
  `check` is its own decision.
- **`AbilityGuard` refusing a visitor on every `Gated` unit**: a `#[field_resolver]` is
  `Gated` and legitimately carries a visitor under a `#[public]` root, and on a gated
  root field the class gate's line, which names the action and the subject, would give
  way to the guard's.
- **Moving the field resolver to `InBand`** to make that sentence true: its posture is
  its operation's, and the line is still lost.

What it leaves: a subgraph whose guards identify no one publishes its SDL to whoever
reaches it, as the federation page warns; `federation: true` is the line that opens it.
A guard of one's own that admits an in-band request decides these units in its own
`check`, where `Posture::Gated` is what it reads.
