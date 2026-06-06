# nest-rs-database

Store-agnostic data-layer extension contract for nestrs — works for SQL,
NoSQL, key-value, or any backend with a handle and an optional notion of
a transaction.

`nest-rs-database` ships **only the seam**:

- `Executor` — the object-safe trait an ORM handle implements (pool,
  transaction, or an enum forwarding to either) so it can ride in the
  ambient `Arc<dyn Executor>`.
- `ExecutorScope` — the enum tagging the ambient executor as `Request`
  vs `Job`, read back by an ORM's `Repo` to fail closed when an HTTP
  path lacks an authorization context.
- `with_request_executor` / `with_job_executor` / `current_executor` /
  `current_executor_scope` — the `tokio::task_local!` plumbing that
  carries "the request's current handle on a unit of work" across the
  framework.

Worker transports (`#[scheduled]`, `#[processor]`) hook in through
`nest_rs_core::JobContext`, which resolves before each job and is where
a driver module installs its job-scope executor.

It is what every driver integration plugs into — SeaORM, sqlx, MongoDB,
Redis-as-store, anything carrying a connection — not itself an ORM or a
client.

The first-class implementation is **`nest-rs-seaorm`** (SeaORM): it ships
`Repo` (row-level filter), `CrudService`, `Bind`, the HTTP mask shaper,
and `DatabaseModule` (the request interceptor that opens the
transaction). Those pieces are SeaORM-specific by design — the leverage
comes from binding tightly to the ORM's query and model types. A future
third-party `nestrs-<technology>` crate (sqlx, diesel, mongodb,
clickhouse, …) reuses this crate's task-locals and lives side-by-side
without touching `nest-rs-core` or any feature code.

## Extension contract

To add a new driver:

1. Implement `Executor` on the type that represents your handle — a
   pool, a transaction, or an enum forwarding to either:

   ```rust
   pub enum MyExecutor { Pool(MyPool), Txn(MyTxn) }

   impl nest_rs_database::Executor for MyExecutor {
       fn as_any(&self) -> &dyn std::any::Any { self }
   }
   ```

2. Ship a `Module` that, for each HTTP request, wraps the handler in
   `nest_rs_database::with_request_executor(Arc::new(your_executor), fut)`.
   For worker transports do the same with `with_job_executor` via
   `nest_rs_core::JobContext` (the `WorkerDbContext` in `nest-rs-seaorm`
   is the reference shape).

3. Provide your own `Repo`-equivalent query API that calls
   `nest_rs_database::current_executor()` and downcasts via
   `executor.as_any().downcast_ref::<MyExecutor>()`. A downcast miss
   is a framework bug (mismatched `Module` + `Repo`); a clear panic in
   a boot test is the documented response, never a silent "no rows".

## What lives in `nest-rs-seaorm`, not here

`Repo<E: EntityTrait>`, `condition_for<E>` (row-level filter), the HTTP
mask shaper, `Bind<S, A>`, `CrudService`, `LoaderScope`, `WsDataContext`
— every piece that couples to SeaORM's `EntityTrait`/`Model` — ships in
`nest-rs-seaorm`. An abstraction over them would lose 80% of their value.
A new driver ships its own row-level-filter equivalent — the
declarative seam is the `Ability::condition_for` API in `nest-rs-authz`,
which is already store-agnostic on the policy side and produces a
SeaORM-typed `Condition` on the SQL side. Mirroring that split for
another store (SQL injection at query time, BSON document filter for
MongoDB, key-prefix filter for KV stores) is the integration's job.

## Transaction commit/rollback

There is **no `Tx` trait shipped here**. The first-party SeaORM
integration manages commit/rollback inside its interceptor against the
concrete `DatabaseTransaction` type — one ORM, hand-managed, no shared
abstraction yet. A future second integration (sqlx, diesel-async, …)
will introduce a real commit/rollback trait once a second implementor
exists to shape it; nestrs would rather ship one honest seam than two
ORMs going through a trait that one of them ignores.
