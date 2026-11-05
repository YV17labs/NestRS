# What a declaration needs is refused at boot, with the line to add

A wiring gap the boot can read off a declaration is refused there, naming the
line that closes it, never left to the first request that reaches it. Three
gaps booted in silence before 7.0's identity audit (2026-10-28, agreed by the
owner's "Tout faire"), and each failed every request it touched:

- **A unit-of-work binding.** An app leaving `SeaOrmDatabaseModule` out booted,
  and every `Repo` call then failed closed — `no ambient database executor` at
  `error`, an opaque `500` on the wire. A host whose generated code runs `Repo`
  now declares the need through the access graph's `HostRequires` entry, keyed
  `Arc<dyn UnitOfWork>`: a `#[crud]` controller or resolver, a route or a
  field under `#[authorize(A, bind = S)]`, and the service an exposed entity
  names (`#[expose(service = S, graphql)]`, whose loaders run `Repo`). One the
  app reaches with no binding is a `MissingDependencyError` naming the binding.
- **A port backend no provider's import binds.** A provider injecting
  `Arc<dyn JobProducer>` with no queue backend was told to "add a provider …
  or seed it at `App::builder()`" — the wrong fix, in kernel words — while the
  port's own `BACKEND_REMEDY` was said only for two backends. The crate owning
  a key's type now submits its remedy keyed by the key's `TypeId`
  (`nest_rs_core::DependencyRemedy`), text only, and `MissingDependencyError`
  carries it (`remedy`): the queue port's for its producer, the database
  port's for its unit of work, `SeaOrmModule::for_root`'s for the pool,
  `RedisModule::for_root`'s for the default instance's connection.
- **A relation's loader owner.** An exposed relation whose target's module the
  serving app did not import booted with a count at `warn`, then every query
  selecting the field erred. Each relation now registers with the object it is
  served on and its loader's owner, and the schema build refuses one the
  schema serves whose owner the app does not reach, naming the field, the
  entity declaring it and the module to import. The count stays, for the
  loaders no served field needs.

**Refused:**

- **Folding the binding into `SeaOrmModule::for_root`** (the audit's
  proposal, an ordinary factory a third-party driver's declaration would
  supersede, `SeaOrmDatabaseModule` removed). The import still chooses: every
  unit over SeaORM's executor, the pool's budget reach widened from injected to
  ambient, the outbox relay attached. A pool reached only by injection — a
  strategy reading its own table, `GuardedPoolModule` in the e2e suite — is a
  shape the framework models, and folding would put every unit of such an app
  over SeaORM's executor. The binding is the `<Vendor><Port>Module` the
  architecture rule names, as `RedisQueueModule` is; the port bindings stay
  (`resource-reports-its-own-health.md`).
- **Refusing every open pool with no binding.** It refuses that legitimate
  shape, and a `warn` on correct wiring is what
  `undeclared-environment-is-production.md` argues against: the refusal reads
  a declared need, never the mere presence of a pool.
- **Refusing a `CrudService` provider as such.** `impl CrudService for X` is
  hand-written and no macro sees it; `#[module]` cannot probe a seaorm trait,
  and seaorm cannot implement a kernel trait for every `S: CrudService` (the
  orphan rule). A service only hand-written code reaches is still noticed at
  its first query, fail-closed, its `error` line naming the binding.
- **Importing a relation's target module automatically.** Discovery that
  mounts a loader is module-gated (`forward-principal.md`): the import is the
  one visible opening, so the boot names it rather than taking it.
- **Reading what the schema serves off its SDL.** A schema whose root has no
  field exports an SDL that does not parse back, and correct apps were
  refused; the schema's names (`Schema::names`) hold the object and the field.
- **A remedy per named instance through the `TypeId` table.** The redis crate
  cannot name a `TypeId` for a marker the app declares, nor can `#[instance]`
  name redis, so a named instance's `RedisConnection<Cache>` injected with no
  `RedisModule::for_root((Cache, …))` keeps the generic sentence; closing it
  takes a probe at the inject site, as a handler parameter's is asked.

## 2026-11-04 — a hand-written reader and the field's own resolver, read by Rust type

Agreed by the owner's "Tout faire (Recommandé)" of 2026-10-31 (identity audit,
second pass). A `#[field_resolver]`'s `&DataLoader<L>` parameter now registers as
a relation does, so a served field whose loader's owner the app does not reach
is refused naming the import, which it still never takes; the parameter binds
through `data_opt`, its absence failing the field with the type on the
operator's line. A `Context` the body reads by hand declares nothing, so no
boot reads it, and the docs teach only the parameter.

Verifying that showed the same gap one step up: `#[ComplexObject]` grafts a field
resolver onto its parent at compile time, so a served parent served the field
whether or not the app reached its resolver — a panic on what the resolver
injects, or, with nothing injected, a body running outside every module the app
imports, which the module gate forbids (`forward-principal.md`). Each
`#[field_resolver]` now registers its resolver, and the schema build refuses the
same way.

**What a served field is, moved.** `Schema::names` is a flat set: a binary links
objects of one GraphQL name it does not all serve, and any field of that name on
any type read as served, which refused correct apps once every field resolver
registered. The check now reads what the reachable resolvers register into a
registry of its own, each object with its Rust type and its fields, against the
registration's `type_name`. The SDL stays refused, for the reason above.

**Refused: a boot refusal for a `Context` body.** Nothing declares it, and reading
one would take parsing a method body; a body that wants the check takes the
parameter.

## 2026-11-05 — the binding imports the pool, which is not the folding refused above

Agreed by the owner's "Tout faire (Recommandé)" of 2026-10-31 (identity audit,
second pass). `SeaOrmDatabaseModule` now imports `SeaOrmModule`, so *a pool opened
with no binding* and *a binding with no pool* stop being the same refusal's two
halves: the second no longer exists, while the need a host declares is still
refused naming the binding. The refusal above still holds: both modules stay, and
an app reaching the pool by injection alone still imports `SeaOrmModule` and puts
no unit over SeaORM's executor.

## 2026-11-05 — a root field reads a loader as a field resolver does

"The docs teach only the parameter" was false of a root field, which had no parameter to teach: `&DataLoader<L>` on a `#[query]` was an `InputType` error, so `ctx.data::<DataLoader<L>>()?` was its one spelling, and with the owner unreached it sent the client ``Data `…DataLoader<…>` does not exist.``, a Rust type path, with no operator line and no boot refusal. `#[query]`, `#[mutation]`, `#[subscription]` and `#[entity]` now take the parameter, left out of the schema's arguments and an entity's key, each submitting a `GraphqlRootLoaderRegistration`. It is read against the resolver rather than the objects a schema serves: an `#[entity]` is no field of any object, and a root field is served exactly when the app reaches its resolver. A `Context` a body reads by hand stays unreadable by the boot, as above; the answer is what moves. `Redacted` reads `Context::data`'s refusal off every error, as it reads async-graphql's other sentences, and answers the opaque error with the type on the operator's line, so no Rust type path reaches a client, whichever data was missing. That line names no field: an error's path holds the client's aliases, which no log line carries, and the field's own `graphql.field` line sits under the same trace. Decided on security and on the developer's ease: the shorter spelling is the checked one.
