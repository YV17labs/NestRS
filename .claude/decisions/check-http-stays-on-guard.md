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
