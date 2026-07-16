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
| `users/` (root) | `entity.rs`/`entities/`, `service.rs`, `dto.rs`/`dtos/`, `command.rs`/`event.rs`, `error.rs`, `module.rs` | `UsersModule` (port) |
| `users/http/` | `controller.rs` | `UsersHttpModule` |
| `users/graphql/` | `resolver.rs` (field + root merged into `UsersResolver`) | `UsersGraphqlModule` |
| `users/ws/` | `gateway.rs` | `UsersWsModule` (imports `WsModule` too) |
| `users/queue/` | `processor.rs` (payload lives at the port) | `UsersQueueModule` |
| `users/schedule/` | `tasks.rs` (`#[scheduled]` host) | `UsersScheduleModule` |
| `users/mcp/` | `tool.rs` | `UsersMcpModule` |

**Each adapter imports `UsersModule` explicitly** — composition, not
inheritance. Importing only the port mounts no endpoint. **No umbrella
module re-exporting every edge**: the app lists the edges it serves, so
imports reflect what the binary actually exposes.

**One `#[module]` per folder.** The DI file is **always** `module.rs`;
**exactly one** `#[module]` struct per file. Multiple modules per feature
⇒ multiple folders.

**One `service.rs` per feature — don't fragment.** Don't split into
`loader.rs`/`credential.rs` unless a second pattern appears twice. Extra
`impl` blocks (`CrudService`, the opt-in `Creatable`/`Updatable`/
`Deletable`, `#[dataloader]`, `#[hooks]`) are macro requirements, not
extra files.

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
