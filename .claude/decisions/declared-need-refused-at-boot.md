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
