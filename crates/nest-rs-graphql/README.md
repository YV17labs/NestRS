# nest-rs-graphql

GraphQL resolver discovery + schema composition for [nestrs](https://nestrs.dev)
apps, built on [async-graphql](https://docs.rs/async-graphql) and served
over HTTP through [async-graphql-poem](https://docs.rs/async-graphql-poem)
on the `nest-rs-http` transport.

`#[resolver]` on an impl block scans `#[query]` / `#[mutation]` / `#[field]`
methods and submits them to a link-time `inventory` registry. The schema
roots `DiscoveredQuery` / `DiscoveredMutation` merge fields from the
registry at runtime (filtered by `ReachableProviders` for module-gating) —
the runtime analog of async-graphql's compile-time `MergedObject`. There
is no central `queries = [...]` list.

async-graphql is nestrs' first-class GraphQL choice. The crate is
intentionally async-graphql-typed: `Context`, `Schema`, `Registry`,
`MetaType`, `OutputType`, `ContainerType`, `DataLoader`, `Extension`,
`SDLExportOptions` all surface as-is to resolvers and macros.

## The engine-agnostic seams

There are two crisp seams in this crate. Both are async-graphql-aware
(they have to be — they bridge poem state into async-graphql state and
re-bind ambient context inside async-graphql's extension pipeline). Both
are documented so a feature can implement them without coupling to authz
or to the ORM:

| Seam | Trait | Implementor today | Purpose |
|------|-------|-------------------|---------|
| Per-operation guard | `OperationGuard` (in `context.rs`) | `nest_rs_authz::graphql::GraphqlAbilityBridge` | Attach principal + install ambient `Ability` around one operation. |
| Per-batch ambient | `BatchContext` (in `loader.rs`) | `nest_rs_seaorm::graphql::LoaderScope` | Re-install executor + ability around a DataLoader batch that async-graphql spawned on a fresh task. |

The lifecycle contract is shared with HTTP and WS: `nest_rs_core::Transport`.
There is **no** separate "GraphQL transport" — the schema self-mounts as
an `HttpEndpointMeta` on `nest-rs-http`, so an alternative HTTP engine is
the entry point an alternative GraphQL integration plugs into.

## Writing an alternative GraphQL engine integration

A community member wanting a [juniper](https://docs.rs/juniper)-backed
integration would write `nest-rs-graphql-juniper` against:

1. The same `Transport` / `HttpEndpointMeta` mount seam (whichever HTTP
   engine the app uses).
2. Its own `#[resolver_juniper]` + `#[query_juniper]` macro pair that
   submits to its own link-time registry (juniper's macro and `Object`
   types are not interchangeable with async-graphql's).
3. Its own `LoaderRegistration` / `ContextSeed` equivalents.
4. Its own version of `OperationGuard` and `BatchContext` — the *shape*
   (a per-operation `before/around` and a per-batch spawner) carries over,
   but the trait signatures bind to juniper's `Context` /
   `LookAheadSelection`.

What stays shared:

- `nest-rs-core` (DI, modules, access graph, `ReachableProviders`).
- `nest-rs-config` (the `NESTRS_GRAPHQL__*` / app-chosen env scheme).
- Every `crates/features/<feature>/` port (services, entities, errors —
  no GraphQL types).
- The pattern: one resolver inventory + one loader inventory + one
  context-seed inventory per resolver crate. The infrastructure
  (inventory traversal, module gating against `ReachableProviders`,
  per-request context bridge) is engine-agnostic in shape and would
  be re-implemented by the alternative crate.

What stays async-graphql-only:

- This crate's `DiscoveredQuery` / `DiscoveredMutation` roots and the
  `merge_type_info` logic — they wire async-graphql's `Registry` /
  `MetaType` model.
- `ResolverRegistration::type_info` and `ResolverRegistration::build`
  return async-graphql metadata and trait objects.
- `LoaderExtensionFactory` is an `async_graphql::extensions::Extension`.
- `ContextEndpoint` builds an `async_graphql::BatchRequest`.

## What this crate exports

- `GraphqlModule`, `GraphqlSetup`, `GraphqlConfig` — activation and
  configuration (path, playground, SDL emit). Self-mounts as
  `HttpEndpointMeta`.
- `OperationGuard`, `BoxFuture` — the per-operation seam
  (`nest_rs_authz::graphql` implements this for the ability bridge).
- `BatchContext`, `BatchFuture`, `BatchSpawner`, `batch_spawner` — the
  per-batch ambient seam (`nest_rs_seaorm::graphql::LoaderScope`
  implements this).
- `ResolverGuard` — per-resolver gate; reads context seeded by
  `OperationGuard`.
- `ContextSeed` — the `inventory`-based bridge that forwards poem
  request state into the async-graphql context. The
  `forward_principal!` macro is the standard producer.
- `ResolverRegistration`, `ResolverObject`, `ResolverKind`,
  `LoaderRegistration` — `inventory` entries `#[resolver]` and
  `#[dataloader]` submit.
- Re-exports: `pub use async_graphql`, `pub use async_graphql_poem`
  (the poem mount adapter), `pub use inventory`,
  `pub use async_trait::async_trait`,
  `pub use nest_rs_graphql_macros::{crud, dataloader, resolver}`.
