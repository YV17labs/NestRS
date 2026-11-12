# A request-scoped forwarder is gated by the guard that produces its value

`forward_principal!` takes the principal type alone, with no module marker.
Discovery that mounts or exposes something — a route, a resolver, a tool, a
loader — is module-gated so a linked crate cannot serve through an app that
never imported it. A request-scoped forwarder mounts and exposes nothing: it
copies a value only if something already put that value on the request. Its gate
is therefore the module-gated guard that produces the value, and a second gate
below it would cost an empty marker struct in every consumer.

It reads as a relaxation and is not one: the condition that activates the
forwarder — the value is present — is strictly narrower than a provider
registration, since a registered marker would fire the forwarder whether or not
anyone authenticated.

## 2026-10-12 — the forwarder copies from the ambient context

A guard no longer writes into the request's extensions: it attaches
(`cx.attach`), and what it attached is ambient for the rest of the unit once
its stage passed. `forward_principal!` copies the principal from there into
the GraphQL context. The gate argument is unchanged: the value is present only
if a module-gated guard attached it, so the forwarder still activates on a
condition narrower than a provider registration.

## 2026-10-30 — the forwarder retires: a handler reads `Ctx<T>` on every edge

Agreed by the owner with the identity audit's breaking changes ("Tout faire",
2026-10-28). `Ctx<T>` moved to core, and every impl half reads it: a route by
its type, `#[operations]`, `#[messages]` and `#[tools]` by its spelling, from
the unit's ambient context, leaving it out of the protocol's arguments. A
resolver therefore takes `claims: Ctx<Claims>`, and `forward_principal!` —
one line every app with GraphQL repeated, copying the principal alone into
async-graphql's context — is removed with the CLI's line that wrote it.

The reasons above still hold, and now for the parameter: what reads the value
mounts and exposes nothing, and its gate is the module-gated guard that
attaches it, a condition narrower than any registration. Nothing replaces the
forwarder, so there is no second gate to weigh. `GraphqlContextSeed` stays for
the framework's own seeds; `ctx.data::<T>()` remains async-graphql's API and is
no longer taught.

## 2026-11-05 — the parameter is `Ambient<T>`

The 2026-10-30 entry's `Ctx<T>` became `nest_rs::core::Ambient<T>` the next day (`ambient-carry.md`, 2026-10-31), so a resolver takes `claims: Ambient<Claims>`. The reasons stand.
