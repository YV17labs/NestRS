# Repo hands out no connection: its selects run on the ambient executor

A service reaches the database through `Repo` alone, and `Repo` hands it no
connection. `Repo::scoped`, `with_tombstones`, `unscoped` and `unscoped_by_id`
return a `RepoSelect<E>`, which implements sea-orm's own `QueryFilter`,
`QueryOrder`, `QuerySelect` and `QueryTrait` and runs on the ambient executor
through `all`, `one`, `count` and `exists`; `insert_unscoped` and
`resolve_unique_slug` read the ambient executor as `update` does. The handle a
unit installs is a crate-private wrapper (`Installed`) whose `as_any` hands out
itself, so a downcast of `nest_rs_database::current_executor()` finds no type a
service can name. Decided by the owner on 2026-10-28 ("Tout faire", the
identity audit's api-1, feat-15 and api-9), security first.

Until then `Repo::conn()` returned the raw `Executor`, which implements
`ConnectionTrait`: every scoped read made a service hold it, and the same
handle ran `Entity::find().all(&conn)` or `active.insert(&conn)` with no
ability filter, through no escape the data-layer rule named. The docs taught
an ability-less read through it. Taking `conn()` away left a second route: the
port's public reader, downcast to the public enum, was the same handle.

The seaorm enum is `SeaOrmExecutor` — a driver carries its own name beside the
port's `Executor` (`architecture.md`) — and `nest-rs-database` keeps the one
install and read API: seaorm's `with_*_executor`, `current_executor`,
`ExecutorScope` and `current_executor_scope` duplicates went, a test installs
`SeaOrmExecutor::Pool(pool).into()` through the port's `with_request_executor`
or `with_job_executor`, and the port's scope-keeping `with_executor` moved to
its `__private`, the in-unit swap the driver's carry path calls.

**Refused:**

- **`Ambient`, an object implementing `ConnectionTrait` that resolves the
  ambient executor per statement** (feat-15's proposal). `Users::find()
  .all(&Ambient)` keeps the bypass under a shorter name.
- **`Repo::raw_conn()`, a named escape for the raw executor.** No caller in the
  tree needs one once the select runs itself; `CrudService`, the keyset page
  and the relation loader live in the crate and read `Repo::conn()`, now
  `pub(crate)`. A raw statement inside a unit is not offered; one that a
  product needs is a named escape with its bar, added then.
- **A typed `SeaOrmExecutor::current()` reader in tier 1.** It reopens the
  handle `conn()` closed. The reader is `__private`, for the crate's own
  suites.
- **Leaving the downcast route open and naming it in the rule.** A public
  reader plus a public handle is the bypass by another name, and a type can
  hold the line instead of review.
- **Two wrapper types, `ScopedSelect` and `UnscopedSelect`.** The escape is
  named at the call (`Repo::unscoped`), which is what a reviewer searches; the
  value it returns runs the same way.
- **The ability's condition applied when the select runs, not when it is
  built.** `build()` would then render SQL the database never receives.
- **`paginate` on the select.** The framework's one pagination is the keyset
  `Repo::page` (`/database/pagination/`); an offset page is `limit`,
  `offset` and `count` on the same select, and sea-orm's `Paginator` borrows a
  connection the select no longer hands out.
- **Projections and joined fetches (`into_tuple`, `find_also_related`) on the
  select.** A joined related entity would come back under the main entity's
  filter alone; a related row comes through its own service's loader, under
  its own. A projection is added with a caller that needs it.

**Limits — what the types leave to review:**

- **A statement rewritten through `query()`.** Each of sea-orm's builder
  traits requires a `query()` handing out the `SelectStatement` as `&mut`, and
  a trait's method cannot be sealed: replaced (`*query() = E::find()
  .into_query()`), cleared (`take()`) or `union`ed with an unfiltered
  `SELECT`, the statement runs past the filter through no named escape. It is
  the one way past the filter through `Repo`; dropping `QueryTrait` alone
  would not close it, since `QueryFilter::query` is the same handle. A
  `.query()` on a `Repo` select is a review trigger, grepped like an escape's
  name.

Implementing sea-orm's builder traits is not the wrapper
`public-dependencies.md` refuses: the builder methods are sea-orm's own, with
their documentation; only where the statement runs is the framework's.
