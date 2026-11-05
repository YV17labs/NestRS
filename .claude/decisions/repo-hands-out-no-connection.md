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
- **The ambient installers.** `nest_rs::authz::with_ability` runs `Repo`
  under the ability it is handed, and `nest_rs::database::with_job_executor`
  grants the job scope to its handle outside every unit — a test, a task
  spawned off one. Inside a unit the installer never widens it: a request,
  or a body continuing one, stays a request, held by the port's tests, since
  a re-install of the unit's own handle under the job scope turned a read
  that failed closed into an unscoped one. Both are named in the data-layer
  rule's escapes; closing them is taking them out of the public API once a
  service test opens its unit under an ability through the testing harness.

Implementing sea-orm's builder traits is not the wrapper
`public-dependencies.md` refuses: the builder methods are sea-orm's own, with
their documentation; only where the statement runs is the framework's.

## 2026-10-31 — a service test runs in a unit of the harness

`TestApp::unit(ability, || svc.list())` and `system_unit(|| …)` (and
`HeadlessApp`'s) run a body in one unit the app's bound `UnitOfWork` opens,
under the ability given or as system work, committed on `Ok`: the demo's
service tests stopped installing `SeaOrmExecutor::Pool(..)` and an ability by
hand. The ambient installers' residual named above loses its last test
caller in the demo; taking them out of the public API is the next branch's.

## 2026-10-31 — no public function widens what a unit runs under

The ambient installers' limit above is closed, with the owner's agreement to
break the API before 7.0 publishes ("Tout faire", 2026-10-28), security
first. `nest_rs::authz::with_ability` only narrows: inside a unit holding an
ability it installs the intersection — rules of both, a subject allowed only
where both allow it, conditions conjoined, columns both expose, either
denial denying, either's anonymity anonymous — in a job, which holds none, the
ability given, and elsewhere nothing, so a read that failed closed still
does. The ability is ambient through `AmbientAbility`, a type only
`nest-rs-authz` builds (its constructor is `__private`, for the testing
harness and the framework's suites), so `with_ambient` and a layer's
`cx.attach` install none: `Ability` no longer implements `AmbientValue`, and
the ability guard is the one place an ability comes from.
`with_request_executor` and `with_job_executor` moved to the port's
`__private`; the testing harness's `TestApp::unit` and `system_unit` open a
service test's unit, and `GraphqlSocketBuilder::ability` subscribes as a
caller.

The two public ways left in are held by behaviour. `run_unit`, the edge port,
tags no scope wider than the unit it opens in, as the installers do: a job
unit opened inside a request runs under the request's. A capture re-entered
inside another unit (`RequestContinuation::scope` and `enter`,
`TaskContext::carry`) holds each value as its `AmbientValue::reenter` returns
there (`ambient-carry.md`, 2026-10-31): the executor's scope no wider than the
running unit's, the ability what `with_ability` would install there, read off
`current_ability`, so a route no mask shapes stays closed. An ability
intersection keeps every condition of both sides, an empty one included:
sea-query renders an empty `any` as FALSE and a negated empty `all` as NOT
TRUE, so a part skipped for being empty read a grant admitting no row as
admitting every row.

**Limit:** outside every unit `run_unit` tags the scope its spec declares —
that is how a queue attempt, a tick, a lifecycle hook and a third-party
consumer open theirs — so feature code holding the binding
(`current_unit_of_work`, an injected `dyn UnitOfWork`) can open a job unit on
a task of its own and read unscoped. Types cannot tell an edge's task from a
feature's; the data-layer rule names it as an escape, read by grepping
`ExecutorScope::Job` in feature code.

**Refused:**

- **`with_ability` moved whole to `__private`, the job given an API of its
  own.** A narrowing cannot widen, so it needs no second name; a job acting
  for a principal is its one service use, and a second function for it would
  be a second way to install an ability.
- **Narrowing `with_ability` and leaving `Ability: AmbientValue`.**
  `with_ambient(Arc::new(ability), fut)` was the same install under the
  kernel's name, and `cx.attach(ability)` in an interceptor an authz decision
  outside a guard.
- **An empty ability installed in a request holding none.** The request's
  reads already fail closed with a line naming the missing guard; an empty
  ability would turn that wiring defect into silent empty answers.
- **The intersection kept as a pair of abilities, each read in turn.** Every
  read of an ability would branch on it; the flat rule set it builds is read
  as any other, and the cost is paid once, by the narrowing.
- **The continuation named as an escape, held by review.** A third-party
  transport and driver re-enter captures, so the functions stay public; each
  value deciding what it is inside another unit holds the rule where review
  would only read it.
- **`RequestContinuation` and `TaskContext` moved to `__private`.** The
  transport and driver pages hand them to third parties, which tier 2 does
  not serve.

## 2026-11-05 — the enum leaves with the handle

A `SeaOrmExecutor` could be named by anyone and obtained by no one, so a tier-1
type had nothing to serve. It moved to `__private` beside its reader, and
`Txn`, which only two framework tests built, is gone
(`visibility-tiers.md`, 2026-11-05). What the tests above still spell,
`SeaOrmExecutor::Pool(pool).into()`, is the suite's plumbing through both
`__private`s.
