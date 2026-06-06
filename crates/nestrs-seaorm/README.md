# nestrs-seaorm

The first-class SeaORM integration for nestrs — the canonical
implementation of the `nestrs-database` extension contract.

Builds on [SeaORM](https://www.sea-ql.org/SeaORM/) and ships everything
the framework's data-layer promise rests on: row-level filtering,
per-request transactions, response masking, and the transport
extractors that make all of it transparent to feature code.

## What this crate ships

- `DatabaseModule::for_root(...)` — owns the `DatabaseConnection`,
  composed in `App::builder()`. Reads `NESTRS_DATABASE__URL` (and the
  rest of `DatabaseConfig`) via the framework's dual-path
  `env-or-pinned` rule.
- `DbContext` — the HTTP interceptor that installs the ambient
  `Executor` task-local around every request, opens a real
  `DatabaseTransaction` on mutating verbs, commits on 2xx/3xx, and
  rolls back on anything else.
- `WorkerDbContext` — the worker-side `JobContext` that installs a
  **pool** executor (no per-job transaction) around every
  `#[scheduled]` / `#[processor]` invocation.
- `Repo<E>` — the single audited query gateway every service goes
  through. Joins the ambient executor; reads the caller's
  `Ability::condition_for` for row-level filtering.
- `CrudService` — the entity service the controllers, resolvers, and
  gateways delegate to. Emits a `nestrs::orm` span for every operation
  (denials at `warn`).
- `Bind<S, A>` — the HTTP route-model extractor (loads + authorizes
  through `CrudService::access`, returns 404 absent / 403 denied).
- `Scope<E, A>` — the HTTP extractor handing a handler the explicit
  `sea_orm::Condition` for a query it builds itself.
- `LoaderScope` (`graphql` feature) — the `BatchContext` that
  re-installs the snapshotted ability + pool executor around each
  `#[dataloader]` batch (async-graphql spawns these on fresh tasks
  where the request task-locals are gone).
- `WsDataContext` (`ws` feature) — the `SocketContext` that
  re-installs the connection's executor + ability around each
  WebSocket message dispatch.
- `DbHealthIndicator` (`health` feature) — the readiness probe that
  pings the pool with `SELECT 1`.

## Composition

A typical HTTP app:

```rust
use nestrs_seaorm::DatabaseModule;

#[module(
    imports = [
        DatabaseModule::for_root(None),
        // … the rest of the app's edge modules
    ],
)]
pub struct AppModule;
```

A pure worker:

```rust
use nestrs_seaorm::DatabaseModule;

#[module(
    imports = [
        DatabaseModule::for_root(None),
        // the worker's `<Feature>QueueModule` + `<Feature>ScheduleModule`s
    ],
)]
pub struct WorkerModule;
```

Importing `DatabaseModule` is the only opt-in step — the request /
worker interceptors mount themselves, and every service that uses
`Repo` inherits row-level filtering, transactions, and the ambient
ability automatically.

## Relationship to `nestrs-database`

`nestrs-database` defines the seam (`Executor` trait, task-locals,
`with_request_executor` / `with_job_executor`). This crate is the
SeaORM implementation. Apps depend on **this crate**; the
abstractions crate is only consumed by integrators writing another ORM
backend.
