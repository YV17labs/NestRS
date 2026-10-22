# A boot refuses one fact at a time, at the earliest phase that sees it

`App::new` cannot drain a factory queue, so a module that queued one is refused
with `UnresolvedFactoryError` before `register` runs, its remedy being
`App::builder()`. A contested declaration is checked ahead of it, since the
collect phase already holds that fact and it survives the remedy. A duplicate
provider is not: it is only seen while `register` runs, so an app that queues a
factory *and* registers a type twice is refused twice, one boot after the other.

**Refused:**

- **Running `register` on the synchronous path before refusing the queue**, to
  report the duplicate first: the register phase then runs without the outputs
  the queued factories would have built, and the first refusal it files is the
  one that hole causes — `SeaOrmDatabaseModule` naming a missing pool — which
  sends the developer after a module that is fine.
- **One error carrying every refusal**: each refusal is a typed error a caller
  matches; a bundle of them is matched by nobody, and its order would still be
  a choice.

Each refusal is a fact the boot can see at the moment it refuses, with the
remedy that fact has, as every other wiring check reports its first case.

## 2026-10-12 — the register phase keeps the order

A module first imported in a `register` whose `collect` queued a factory is
now refused at that import (`LateFactoryError`), as a refusal of its own: a
later `register` providing the type would otherwise hide it, and a refusal
the missing output causes afterwards reads after it. A late `collect` can
also declare a type again. That contest is read ahead of every refusal, a late
import's included, since it survives the remedy of each and no refusal causes
one.

**Refused:** passing over the late refusal only when the same `collect`
contested. An earlier late import's refusal would still have been read
first.

## 2026-10-12 — after the register phase, the access graph before a provider never built

A provider a `#[module]` lists and the register phase could not build is now
refused at the seal (`UnbuiltProviderError`), after the access graph, whose
remedy is more specific: a dependency outside the module's imports is also
absent when the provider would have been built, so one fact is seen twice, and
the access graph names the module to import. A cycle among one module's
providers is still refused in the register phase that sees it, now also when
another provider of the module waits on something outside it. A cycle among
request-scoped and transient providers, which the register phase never orders,
is refused at the seal right after the access graph, before any stall: no
request could resolve it in any composition. A cycle among singletons of
modules importing each other shows the register phase only a provider never
built, or one built without an optional dependency, so it is walked from
those next, and refused (`ProviderCycleError`) before either: their remedy, a
module both import, would only lead to it. A provider built without an
optional dependency the boot registered afterwards
(`LateOptionalDependencyError`) reads after a provider never built: a missing
provider is the larger hole, and both carry the same causes.

**Refused:** refusing the stall in the register phase, where `#[module]` sees
it. Its cause — an import cycle, or a registration made later in the boot — is
known only once every module has registered, and the access graph would then
never be read first.

## 2026-10-15 — an override nothing claims, after the register phase's refusals

A test override is installed before any module collects, and whatever would
register its type claims it. One nothing claimed (`UnclaimedOverrideError`)
is read once the register phase's own refusals are: a module's refusal, a
late factory or a duplicate can leave the provider that would have claimed it
unregistered, and their remedies then claim it. A consumer of a key an
override left absent — the other key of a `Foo as dyn Trait` binding
overridden through one — is refused at the seal after the access graph and
the cycles, before a provider never built (`OverriddenDependencyError`): that
consumer, a singleton, would otherwise also stall, and the stall's causes,
an import cycle or a later registration, would send the developer after a
module that is fine.

**Refused:** reading the unclaimed override before the register phase runs.
Only the register phase shows which module providers claim, so the refusal
would fire on every override of a module's provider.

## 2026-10-18 — a container built by hand refuses at its build, in the boot's order

`ContainerBuilder::build` now returns what the builder holds that the boot
would refuse, one fact at a time and in the boot's order: what the register
phase leaves (a contested declaration, the refusal filed, a factory nothing
drains, a module collected alone, a type registered twice), then what the seal
reads off the builder's modules, which stand for the roots (a cycle among
request-scoped and transient providers, one among singletons of modules
importing each other, a provider never built, one built before an optional
dependency registered after it, a budget at or past a net reaching it). It
used to freeze what registered and drop the rest, so a unit test found a
module's providers absent and no error saying why. The refusal is typed,
`ContainerError`, one variant per refusal the kernel names and `Refused` for a
`register`'s own; the boot flattens it back, so what a boot error downcasts to
is unchanged. A budget is read last, where the seal reads it: the boot also
reads it as soon as the resource exists, so that a factory's I/O does not fail
on something else first, and a builder made by hand runs no factory.

A builder made by hand also says so: a factory queued on it is refused with
that builder's edit — provide the type first, and each trait object a binding
derives from it, or boot through `App::builder()`; a declaration, which no
value provided there stands in for, through the boot alone — rather than the
late-import remedy a boot's register phase earns; a registration outside any
import names the builder rather than `App::builder().provide…`; and a
provider never built names the order of the builder's calls
(`UnbuiltCause::ImportedFirst`).

**Refused:**

- **`anyhow::Result<Container>`**, the boot's own currency: a library returns a
  typed error, and a caller matches the refusal by its variant.
- **Reading the access graph at `build`**: it holds a provider to its
  module's imports and to what is global — a seed or a factory's output — and
  a builder made by hand cannot tell a seed from what its modules register,
  both being calls on it in the order they are written. Read there, it would
  either take every registration as global and check nothing, or take none
  and refuse a test whose module reaches a value the test seeded. A container
  built by hand stays the unchecked escape hatch for it; the boot reads it.
- **Running the wirings at `build`**: a wiring fills its registry from the
  reachable set and the provider order the boot's seal hands the container,
  which a container built by hand never holds.

## 2026-10-22 — a container built by hand holds its seeds to their checks, after a contest

`ContainerBuilder::build` now runs the check each queued factory declares on
a seed of its output (`__private::check_seed`, which `ConfigModule` declares
for each config it queues), where the boot's factory phase runs it: after a
contested declaration, before what the register phase leaves. A builder made
by hand drains no factory, so a seeded config skipped its `Config::check` and
passed a driver's own suite only to fail the app's boot. The refusal is
`ContainerError::SeedRefused`, carrying the check's own error, which the boot
returns unwrapped.

**Refused:** running the check at `provide`, which cannot know whether a
factory of that type will be queued, nor read the configs a check compares
against, registered later in the builder's calls.
