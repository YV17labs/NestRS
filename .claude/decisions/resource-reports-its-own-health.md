# A resource reports its own health: the module opening it attaches its indicator

A resource the framework opens — the SeaORM pool, each Redis instance, the
object store — ships its readiness indicator in its own crate, behind its
`health` feature, and the module opening the resource attaches it from its
`register`: `SeaOrmModule::for_root`, `RedisModule::for_root` for each instance,
`StorageModule`. An app importing `HealthModule` checks every resource it or
its features opened, and writes nothing else. Decided by the owner on
2026-10-27, on the demo's `RedisHealthModule, RedisHealthModule::<Transcodes>,`:
health is a cross-cutting concern, and one the developer must hand-manage is a
framework defect (the project map, *Identity*).

The attachment sits in the module the resource's `for_root` imports by type
(`SeaOrmOpening`, `RedisOpening<I>`, `StorageModule`), whose phases run once
per app: two features opening one instance report it once, where a second
report would fail the boot's name check. The indicator stays metadata
(`IndicatorEntry`), so an app without `HealthModule` reads none and pays
nothing on any request.

**Refused:**

- **A health binding module per resource** — `SeaOrmHealthModule` since 6.0,
  generalised to `RedisHealthModule<I>` and `StorageHealthModule` on
  2026-10-19 — removed in 7.0 without an alias. The app had to import one
  per resource beside `HealthModule` — including the resources its features
  opened, which it had to know of — and a feature adding a resource left
  every app's readiness blind to it, silently. The rule that prescribed
  them carried the defect; the port bindings that choose a backend for a port
  (`SeaOrmDatabaseModule`, `RedisQueueModule`, `RedisScheduleModule`) are a
  different thing and stay.
- **`HealthModule` discovering resources by type** (probing whatever
  `DatabaseConnection` or `RedisConnection<I>` the container holds). The
  health crate would name every vendor, a package's resource could not join,
  and the key would no longer be read off the crate owning the resource.
- **An opt-out per resource.** An indicator exists only for a resource the
  boot proves, and reads what the proof reads: a resource that can stop the
  boot can take a replica out of rotation. One exempt from the proof ships
  none (`container.md`).

## 2026-11-05 — the module opening the resource is the resource's own

Since the bare import reads (`bare-import-reads-for-root-pins.md`), `SeaOrmModule` and `RedisModule<I>` implement `Module` by type and attach the indicator from their own `register`; `SeaOrmOpening` and `RedisOpening<I>` are gone. Storage keeps a private `StorageOpening`, which `StorageModule` imports. Each still runs once per app.
