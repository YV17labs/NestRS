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
