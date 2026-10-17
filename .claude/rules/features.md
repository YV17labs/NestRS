---
paths:
  - "demo/crates/features/**/*.rs"
  - "demo/crates/features/**/*.toml"
---

# Product features — port and adapters

`demo/crates/features/` holds the product's vertical slices, hexagonal per
slice: the port at the feature root, one adapter folder per edge.

## The bar

- **A new CRUD feature is at most 60 lines of hand-written glue** beyond the
  entity's column declarations; `orgs/` is the measurement. When that breaks,
  open an issue — never rewrite the boilerplate.
- **Adding a feature is copying `users/`**, plus the two edits a copy cannot
  carry: `pub mod <feature>;` in `lib.rs` and the `<Feature><Edge>Module`
  entry in the serving app's `module.rs`. `nestrs g feature/resource/<edge>`
  does all three. If the copy is not enough, fix the exemplar; never invent a
  second pattern.
- **Security is wired by composition, not ceremony.** Importing
  `SeaOrmModule::for_root`, `SeaOrmDatabaseModule` and `Authz<Edge>Module`
  turns on row filtering, the transaction scope and response masking. Guards
  still bind per route: the principal source is a policy decision.

## Layout

The port lives at the feature **root**, never in a `core/` folder.

| Path | Contents | Module |
|---|---|---|
| `users/` | `mod.rs`, `entity.rs`/`entities/`, `service.rs`/`services/`, `dto.rs`/`dtos/`, `command.rs`/`event.rs`, `config.rs`, `error.rs`, `guard.rs` (only when two adapters bind it), `module.rs` | `UsersModule` (port) |
| `users/http/` | `controller.rs` | `UsersHttpModule` |
| `users/graphql/` | `resolver.rs` (field and root operations in one `UsersResolver`) | `UsersGraphqlModule` |
| `users/ws/` | `gateway.rs` | `UsersWsModule` |
| `users/queue/` | `processor.rs` (its payload lives at the port) | `UsersQueueModule` |
| `users/schedule/` | `tasks.rs` | `UsersScheduleModule` |
| `users/mcp/` | `tool.rs` | `UsersMcpModule` |
| `users/events/` | `listener.rs` | `UsersEventsModule` |

**Each adapter imports its port explicitly** — composition, not inheritance.
Importing only the port mounts nothing, and no module re-exports every edge:
the app lists the edges it serves, so its imports are what the binary exposes.

**A feature that emits events declares its span target at its root** —
`pub const TARGET: &str = "features::<feature>";` in its `mod.rs` — and every
event names `crate::<feature>::TARGET`, never a literal (`CLAUDE.md`,
*Observability*). A feature that emits nothing declares none.

**The adapter shape is invariant.** One edge, one adapter folder, one
`<Feature><Edge>Module`, for every feature. A product never inverts it into a
single top-level edge folder injecting every service: that trades the module
gate — an app importing exactly the edges it serves — for a god-adapter no app
can subset, which hides every route or tool behind one provider in the access
graph. No adapter there could be named: `features` is a container, never a
subject (`architecture.md`), and review refuses an edge folder directly under
`features/src/`.

**A transport that cannot host two features at one mount point is a framework
defect**, unless `edges.md` argues it (a WS gateway owns its path). Report it
and keep the shape. `demo/apps/assistant` is the witness
for MCP: `audio` and `users` share `/mcp` through bare `#[mcp]` hosts, and
`posts` mounts its own path.

**A feature with a `config.rs` imports `ConfigModule::for_feature::<C>()` and
writes no `for_root`**: the in-code path for a config you own is its
`impl Default` (`architecture.md`, *Configuration*). `oauth/module.rs` and
`audio/schedule/module.rs` are the exemplars.

## One `service.rs` per feature

Extra `impl` blocks (`CrudService`, `Creatable`/`Updatable`/`Deletable`,
`#[dataloader]`, `#[hooks]`) are macro requirements, not extra files.
Splitting is a last resort, chosen by `architecture.md`'s three provider
questions, never by file size:

- **The extracted thing dispatches nothing and owns no domain logic** — a
  factory, a client, an enum. It is a custom-provider or vocabulary file named
  for what it is, beside `service.rs`. The common case; the service count stays
  one.
- **The slice owns two bodies of domain logic.** Then `services/`, one
  bare-named file each (`services/input.rs` holds `InputService`), re-exported
  flat. Two services because a name was hard to choose is a mis-modelled slice.

## Errors — the framework owns the plumbing

A feature never redefines `nest_rs_seaorm::ServiceError` or the authn errors
(`AuthError`, `CredentialError`, `TokenError`). It writes its own only for a
domain-specific wire contract or a security-opaque variant, in `error.rs`.

## Transfer objects — where they live

The suffixes are `architecture.md`'s (*Transfer objects*). Placement:

- **A REST body** (`Dto`), **a queue `Command`** and **a published `Event`**
  live at the port: a queue payload is a producer↔worker contract, so the
  `queue/` adapter's processor imports it. A scaffolded job is a `Command`;
  choose `Event` only to broadcast a fact.
- **A WS payload** (`Dto`) lives with the gateway's feature; **a hand-written
  GraphQL input** in `graphql/input.rs` or `graphql/inputs/`; **a GraphQL
  output** is the object type itself (or a `Payload` wrapper), with the
  resolver.
- One type or several is `architecture.md`'s *Several of the same role*;
  events never take a folder, since `events/` is the edge.

**The entity's derived forms are the exception.** `Create<E>` / `Update<E>`
are at once the service's input, the GraphQL `input` and the REST body, so no
single boundary suffix fits; they live inside the entity's `#[expose]` block
(`create = CreateUser`), and the SDL reads `input CreateUser` on purpose.
Hand-written transfer objects keep their suffix. Do not split per transport
without a genuine need.

## GraphQL composition is discovered

Each `#[operations]` block submits its objects to `inventory`, merged into the
schema at boot. The resolver struct is still listed in `providers`, for the
access contract. Batch field fetches with `#[dataloader]`.

## Exemplars

- **`src/users/`** — the reference feature.
- **`src/orgs/`** — the full HTTP CRUD slice the bar is measured on.
- **`src/posts/`** — the tutorial feature.
- **`src/notifications/schedule/`** — work that belongs to the deployment, not
  the process: `replicas = "one"`, hosted by the worker beside the Redis
  schedule binding, so scaling never multiplies it. `audio`'s transcode seed is
  deployment work too. A job about the process itself — `audio`'s heartbeat —
  writes `replicas = "each"`, the default spelled out, so the decision is read
  rather than inferred.
