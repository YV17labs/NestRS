---
paths:
  - "**/service.rs"
  - "**/entity.rs"
  - "**/entities/**/*.rs"
  - "crates/nest-rs-seaorm/**/*.rs"
  - "crates/nest-rs-database/**/*.rs"
  - "crates/nest-rs-resource/**/*.rs"
  - "crates/nest-rs-resource-macros/**/*.rs"
  - "demo/crates/migrations/**/*.rs"
---

# Data layer — transparent security + transactions

## The hard invariant

**Every data access goes through a service; a service reaches the DB
only through `Repo`.**

`CrudService` is the entity's API and the single audited choke point —
controllers, resolvers, gateways and dataloader resolver code
**delegate, never touch `Repo` or the ORM directly**.
`CrudService::list`/`page`/`access`/`create`/`update`/`delete` go through
`Repo`, emitting `nest_rs::orm` spans (denials at `warn`). `Repo` runs
every query against the ambient executor and filters reads **and** by-id
writes by `condition_for` from the ambient ability (no ability ⇒ `TRUE`,
unscoped). Route-model binding goes through the service (`Bind`/`bind`
delegate to `CrudService::access`).

### The only sanctioned bypasses — there are no others

- **`Repo::unscoped()` / `unscoped_by_id()`** (still *inside* `Repo`),
  for exactly two paths: pre-authentication credential lookup (no
  principal yet ⇒ no ability), and `CrudService::access` (must
  distinguish `Denied` from `Missing`, so it filters by ability
  explicitly after the unscoped load).
- **A truly contextless path** (a shutdown hook) keeps an injected
  `Arc<DatabaseConnection>` — the only `Repo`-*less* bypass, because
  there is no executor at all.
- **Signature-authenticated webhook ingress** is **reserved but
  unimplemented** — no webhook route or signature check exists. See
  `Repo::unscoped`'s doc for the bar a real one must clear before
  shipping a `#[public]` + `unscoped` webhook.

Every other read uses `scoped`/`all`/`find_by_id`, which apply the
ambient ability `WHERE`.

## Two request-scoped `task_local!`s

Singletons have no other way to read per-request state:

- **executor** — the `task_local!` seam + `Executor` trait live in
  `nest-rs-database`; `nest-rs-seaorm` supplies the concrete `Executor`
  (pool or transaction).
- **ability** — `nest-rs-authz` ambient `Arc<Ability>`.

**Install depths.** *Executor* via the auto-registered `DbContext`
interceptor (just import `DatabaseModule`) — innermost transport band
(−10), wrapping routing, so it covers controllers and self-mounts alike.
Safe methods run on the pool; mutating methods get a **lazy**
transaction (`Executor::Lazy` — `BEGIN` deferred to the first data-layer
touch) — commit on 2xx/3xx, rollback otherwise **and** on any
`MappedError`-tagged response. Guards run *inside* it (post-routing): a
denied mutation never touches the data layer, so it opens **no**
transaction at all — fail-secure holds at zero `BEGIN`/`ROLLBACK` cost.

*Ability* installs inside per-route guards via the `#[routes]` shaper —
the only seam that runs after `AbilityGuard` and still wraps the handler,
keeping `nest-rs-http` unaware of authz/ORM.

## Write capability is segregated, never a placeholder

`CrudService` carries only the **read** half (`list`/`page`/`access` +
the entity's helpers) — every resource implements it. The write half
lives in three **opt-in** traits a resource implements only when it
genuinely offers the operation: `Creatable` (`type Create` + `create`),
`Updatable` (`type Update` + `update`), `Deletable` (`delete`).

A read-only resource (a relation, a projection, an append-only log)
implements just `CrudService` and declares **no** `Create`/`Update` type
— there is no `struct … { _unused }` stub and no no-op
`apply_to`/`into_active_model` to write.

**`#[crud]` generates only the operations a resource has.** `ops = [list,
get, delete]` synthesises exactly those (HTTP *and* GraphQL); omit `ops`
for all five. A `create`/`update` op requires its input type (`create =`
/ `update =`) **and** the service's `Creatable`/`Updatable` impl;
`delete` requires `Deletable`. Listing an op without its input type is a
**compile error** (named in the diagnostic), and an op whose trait the
service doesn't implement fails to resolve — a forgotten or impossible
operation is a build break, never a silent no-op mutation on the wire.

## Exposure is opt-in

A column crosses HTTP/GraphQL/WS **only** with `#[expose]`; silence =
hidden. Fail-secure on schema evolution: a column added by a later
migration never leaks by omission. The entity *is* the wire contract —
no hand-written per-transport DTO to forget to update.

## Response masking — one shared core, two transports

`nest-rs-authz` `wire_mask`, value-level and **fail-closed**. After
success: parse the wire JSON → build `Model` via `wire_to_model` (filling
the **unexposed** columns the wire DTO omits, from `impl
WireModelDefaults for Entity` emitted by the macro) → `Ability::mask` /
`mask_many` → **retain only the exposed wire keys**
(`retain_static_keys`/`retain_body_keys` — unrestricted field grants
can't leak unexposed columns). Handlers return the `#[expose]` output
(e.g. `Json<User>`), not `Model`. An irreconcilable body ⇒ fail
**closed** (HTTP `500`, GraphQL error).

Reconstruction needs a default for every unexposed column: the macro
provides one for safe scalars (`String`/`Option`/`bool`/numbers); a
hidden column of a type it can't default (`Uuid`, timestamps, `Decimal`,
custom enums) needs a hand-written `impl WireModelDefaults` — **so
columns an ability rule predicates on are best left exposed.**

- **HTTP**: the `Authorize<A, E>` extractor in a handler's signature is
  the arming declaration — `#[routes]` installs the response shaper
  (ambient ability + masking) when it sees it. **It is not dead code:**
  removing the `_authz: Authorize<…>` parameter disarms masking for that
  route.
- **GraphQL**: `#[authorize(Action, Entity)]` beside a
  `#[query]`/`#[mutation]` is the same declaration — `#[resolver]` emits
  the class gate before the call and `masked_value_for` around the
  returned value (wire DTO, `Option`, `Vec`; scalars pass). `unmasked`
  opts a custom shape (cursor connection) out of the automatic mask;
  `masked_output_for` is the manual primitive it pairs with.
  **One schema-typed caveat HTTP doesn't have:** GraphQL cannot ship a
  masked-out **non-nullable** field (HTTP just omits the key), so the
  whole operation fails closed — a column a field-grant may mask should
  be `Option` on the entity (nullable on the wire).

## Extractors

Two HTTP extractors: **`Bind<S, A>`** (parse id → load + authorize via
the service: 404 absent, 403 denied) and **`Scope<E, A>`** (explicit
`Condition` for hand-built queries). Routes using `Bind` must also bind
an `AbilityGuard`.

Same transparency past HTTP via authz/ORM-agnostic seams. `nest-rs-authz`
exposes authz bridges behind features — `http` (`Authorize`,
`AbilityGuard`, `Scope`), `graphql` (`GraphqlAbilityBridge`, `authorize`,
`ability`), `mcp` (`McpAbilityBridge`; masking inside a tool body is the
transport-free `masked_output_ambient`); data-layer bridges live in `nest-rs-seaorm` behind matching
`http`/`graphql`/`ws`/`mcp` features (`Bind`, GraphQL `bind`, `LoaderScope`,
`WsDataContext`, `McpDataContext`) — **the split avoids a circular dep.**

- GraphQL `OperationGuard` = `GraphqlAbilityBridge` and MCP
  `McpOperationGuard` = `McpAbilityBridge` — both run the guard chain in-band
  (`run_ability_chain`, the single authn→authz ordering) and install the
  caller's ability from their `around`, so the **guard** is what scopes an
  operation on either transport, with or without a data context.
- `BatchContext` = `LoaderScope` (snapshots ability + pool executor
  around each off-task dataloader batch).
- WS `SocketContext` = `WsDataContext` and MCP `McpToolContext` =
  `McpDataContext` — both re-install ability + a **lazy** executor per
  dispatch through one shared `dispatch::with_data_context`, so their
  commit/rollback semantics cannot drift apart. A read-only message or tool
  opens no transaction; a writing one commits on success, rolls back on the
  transport's error shape.
- **Worker transports** install the pool via the orm-agnostic
  `JobContext` (`WorkerDbContext`, auto-bound by `DatabaseModule`) —
  system work ⇒ no ability ⇒ unscoped, correct.

## Dataloaders and relations

**`#[dataloader]` batch methods** live on the service, use `Repo`, and
return `Result<HashMap<…>, E>` (infallible only when they truly cannot
fail). **Never map a DB error to an empty batch.**

**Relations resolve themselves.** A SeaORM `#[sea_orm(belongs_to, …)]`
or `#[sea_orm(has_many)]` field **marked `#[expose]`** on an `#[expose]`d
entity becomes a GraphQL field auto-resolved by a dataloader.
`#[expose(name = "…", service = <Path>)]` emits the PK loader
(`<Service>ById`) on the service for every entity, the FK loader
(`<Service>By<FkCol>`) per `belongs_to` on the FK-owning side, the
`PkLoadable` / `RelatedTo<Parent>` impls that let the inverse side reach
the loader **without naming the other service**, and a `#[ComplexObject]`
field resolver on the wire DTO. Every batch goes through
`Repo::scoped(Action::Read)`, so an `Ability` filter applies row-level as
on any other read.

Omitting `#[expose]` on a single relation opts that field out — write a
`#[field_resolver]` for a custom shape (cursor connection, extra filter).

**Cross-entity rule:** a service touching another entity injects that
entity's service; **the FK loader is part of its owner's service, never
the consumer's**.

**One caveat:** async-graphql allows at most one `#[ComplexObject]` per
wire type, so a custom `#[field_resolver]` on the resolver cannot live
next to an auto-resolved relation on the same entity — pick one source
per `ComplexObject`.
