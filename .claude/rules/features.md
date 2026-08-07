---
paths:
  - "demo/crates/features/**/*.rs"
  - "demo/crates/features/**/*.toml"
---

# Product features — port + adapters

`demo/crates/features/` holds product vertical slices. **Hexagonal per
slice**: the port at the feature root, one adapter sub-folder per
transport.

## North Star

- **A new CRUD feature is ≤ 60 lines of hand-written glue** beyond the
  entity's own column declarations (measured on `orgs/`: ~30 non-entity
  body lines for a full HTTP CRUD slice). **When that breaks, open an
  issue — don't rewrite the boilerplate.**
- **Adding a feature = copying `users/`** — plus the two wiring edits the
  copy can't carry: `pub mod <feature>;` in `features/src/lib.rs` and the
  `<Feature><Edge>Module` entry in the serving app's `module.rs`.
  `nestrs g feature/resource/<transport>` does all three.
  **If the copy isn't enough, fix the exemplar — don't invent a second
  pattern.**
- **Security is wired by composition, not ceremony.** Importing
  `DatabaseModule` + `Authz<Edge>Module` activates row-level filtering,
  transaction scope and response masking. Handlers opt *out* by not
  importing. Guards still bind explicitly per route — the principal
  source is a policy decision.

## Layout

The port lives at the **root** — not in a `core/` sub-folder. Deliberate.

| Path | Contents | Module struct |
|---|---|---|
| `users/` (root) | `entity.rs`/`entities/`, `service.rs`/`services/`, `dto.rs`/`dtos/`, `command.rs`/`event.rs`, `config.rs`, `error.rs`, `module.rs` | `UsersModule` (port) |
| `users/http/` | `controller.rs` | `UsersHttpModule` |
| `users/graphql/` | `resolver.rs` (field + root merged into `UsersResolver`) | `UsersGraphqlModule` |
| `users/ws/` | `gateway.rs` | `UsersWsModule` (imports `AuthzWsModule`, which brings `WsModule` transitively) |
| `users/queue/` | `processor.rs` (payload lives at the port) | `UsersQueueModule` |
| `users/schedule/` | `tasks.rs` (`#[scheduled]` host) | `UsersScheduleModule` |
| `users/mcp/` | `tool.rs` | `UsersMcpModule` |
| `users/events/` | `listener.rs` (event listener host) | `UsersEventsModule` |

**Each adapter imports `UsersModule` explicitly** — composition, not
inheritance. Importing only the port mounts no endpoint. **No umbrella
module re-exporting every edge**: the app lists the edges it serves, so
imports reflect what the binary actually exposes.

### The adapter shape is invariant

One transport, one adapter sub-folder, one `<Feature><Edge>Module` — for
every feature, always. **A product never inverts it** into a single
top-level edge folder injecting every domain service: that trades the
module gate — an app importing exactly the edges it serves — for a
god-adapter no app can subset, and it hides every tool or route behind
one provider in the access graph.

**A transport that cannot host two features at one mount point is a
framework defect.** Report it and keep the shape; it is never a licence
to invert. None is open today — MCP was the last one, and it is closed:
several `#[mcp(path = "/mcp")]` hosts aggregate onto one endpoint, so a
product serving several domains at the single URL its clients point at
still writes one `mcp/` adapter per feature. `demo/apps/assistant` is
the witness (`audio` + `users` on `/mcp`, `posts` on `/posts/mcp`).

**One `#[module]` per folder.** The DI file is **always** `module.rs`;
**exactly one** `#[module]` struct per file. Multiple modules per feature
⇒ multiple folders.

**One `service.rs` per feature — don't fragment.** Extra `impl` blocks
(`CrudService`, the opt-in `Creatable`/`Updatable`/`Deletable`,
`#[dataloader]`, `#[hooks]`) are macro requirements, not extra files.

Splitting is a **last** resort, and it takes one of two shapes — pick by
the three questions in `architecture.md`, never by file size:

- **The extracted thing dispatches nothing and owns no domain logic**
  (a factory, a client, a seam, an enum). It is a custom-provider or
  vocabulary file named for what it is, beside `service.rs`. This is the
  common case, and it leaves the service count at one.
- **The slice genuinely owns two services**, each with domain logic of
  its own. Then and only then: `services/`, one **bare-named** file per
  service (`services/input.rs` → `InputService`), flat re-export from
  `mod.rs`. Two services because a name was hard to choose is a
  mis-modeled slice, not a folder.

## Errors — the framework owns the plumbing

A feature **never** redefines `nest_rs_seaorm::ServiceError`, or
`nest_rs_authn::AuthError`/`CredentialError`/`TokenError`. Features write
their own errors only for genuinely **domain-specific wire contracts** or
**security-opaque variants** — in `error.rs`, never as scattered enums
inside `service.rs`.

## Transfer objects — named for the boundary they cross

Each layer speaks its native vocabulary. **The suffix is the boundary**,
not a generic "it moves data" — `…Job` / `…Response` / a blanket `…Dto`
are all wrong.

| Kind | Suffix | Where |
|---|---|---|
| REST body (request/response) | **`Dto`** — `LoginDto`, `AccessTokenDto` | port: `dto.rs` / `dtos/` |
| Queue payload, imperative ("do X" → one handler, idempotent, replayable; verb-led) | **`Command`** — `TranscodeCommand` | port: `command.rs` / `commands/` |
| Queue payload, published fact ("X happened" → many consumers; past-tense) | **`Event`** — `OrderPlacedEvent` | port: `event.rs` / `events/` |
| WS message payload (the `data` of an envelope, either direction) | **`Dto`** — `SendMessageDto`, `ChatMessageDto` | with the gateway's feature |
| GraphQL input, hand-written | **`Input`** | `graphql/input.rs` / `graphql/inputs/` |
| GraphQL output | the object type itself (bare, or `Payload` for a wrapper) | with the resolver |

A **queue payload is a producer↔worker contract**, so it lives at the
**port** (feature root), never in the consumer-side `queue/` adapter —
the `processor.rs` imports it. A scaffolded job defaults to a `Command`
(the common case); choose `Event` only when broadcasting a fact.

The role word is carried by **both** the type and its file, and placement
mirrors the entity rule: one → the bare file, two or more → a pluralized
directory (one `<snake>_<role>.rs` per type, flat re-export from
`mod.rs`).

### The entity exception

The entity and its derived CRUD forms are the exception. The entity stays
`Model` in `entity.rs`; its `#[expose]`d wire struct keeps the **bare
entity name** (the entity *is* the wire contract); and the
macro-generated `Create<E>` / `Update<E>` are **bare too**.

Why: a CRUD shape derived from the entity has no *single* boundary — one
Rust struct is at once the service's `Create`/`Update` type
(transport-agnostic), the GraphQL `input`, and the REST body. A transfer
suffix would be wrong at the service layer and would give a
non-idiomatic `input Create<E>Dto`. So it lives inside the entity's
`#[expose]` block (`create = CreateUser`), not a separate file. The
resulting SDL reads `input CreateUser` — deliberate.

Hand-written transfer objects keep their boundary suffix; **only the
entity-derived forms drop it.** Do not split per transport unless a
genuine need appears.

## GraphQL composition is discovered, not listed

Each `#[resolver]` submits its objects to `inventory`, merged into the
schema at boot. The resolver struct is still listed in `providers` — for
the access contract only. Batch field fetches with `#[dataloader]`
(request-scoped) to avoid N+1.

## Exemplars

- **`src/users/`** — reference feature. Copy before inventing.
- **`src/orgs/`** — the ~30-line full HTTP CRUD slice (the North Star
  measurement).
- **`src/posts/`** — tutorial feature exemplar.
