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

## 2026-10-22 — a scoped layer a provider-hosted site binds, with the scope violations

A layer a `#[process]`, `#[scheduled]` or `#[on_event]` site binds is read by
the access graph from its `HostLayers` entry, and a request-scoped or
transient one is now refused there (`ScopedLayerError`), where a singleton
injecting such a provider is (`ScopeViolationError`), before the reach of any
dependency: a site resolves its layers once, when it composes at boot. A
request-scoped one is never built there: it used to pass the graph and be
refused by the site's composition with "no imported module provides it", a
remedy that is false when a module provides it request-scoped — at the wiring
step for a listener, and at the transport's configure, after the boot, for a
job and a tick. A transient one was built there once and shared by every run,
so an app binding one booted and ran it as a singleton; that boot now fails,
as a transient layer on a controller, a gateway or a resolver already did
(`ScopeViolationError`).

**Refused:** naming the site's method: the `HostLayers` entry carries its
host, which a macro ABI change would have to widen for every decorator.

## 2026-10-31 — one boot path

`App::new` and its `UnresolvedFactoryError` are gone (the identity audit,
agreed under the owner's "Tout faire" of 2026-10-28): every framework module
owning a config queues a factory, so the synchronous boot refused every real
app, and `App::builder()` was the remedy it named. The first section's
ordering argument stands for the one boot left: a contested declaration is
still read ahead of the factories.

## 2026-10-31 — a scoped layer in an app-wide pool

A pool's declaration (`GlobalGuards<(…)>` and its siblings) holds its layers
as a site's host does, not as injections: what it attaches at register joins
the `HostLayers` entries the seal reads, so a request-scoped or transient
layer in a pool fails with `ScopedLayerError`, naming the layer and the
declaration, and the reach of each layer is checked as before. Read as
injections, a declaration — a singleton — was refused with
`ScopeViolationError`, whose two remedies (`Scoped<T>`, a request-scoped
declaration) do not exist for a pool; its one remedy is the site's, a
singleton layer. A transient global layer had booted until then, built once
at mount and shared by every request.

**Refused:** a field on `ProviderDescriptor` filled by `#[module]` from a new
`Discoverable` method — a macro ABI change and every hand-built descriptor —
when the seal already runs after the register phase that records the pool.

## 2026-11-04 — a pool registers what it lists, after every module

Before: a pooled layer had to be a reached provider as well, so a root owning
one wrote its type twice in one list. Now (the owner's "Tout faire (Recommandé)"
of 2026-10-31) the boot registers each listed layer nothing else registered, once
the register phase's modules are done and before the register phase's refusals
are read, so a duplicate, a stall or a refusal it files is read in the order
above and the access graph still runs after it. Waiting for every module is what
makes "already registered" a fact rather than a guess: a seed, a factory, a
sibling listed later, a second root and a test's override all stand with no read
of the descriptors, and a layer's optional dependency is never late. A layer a
module lists and could not build is not registered in its place: the stall stays,
and with it the cause it names (an import cycle, a late registration), which a
pool building the layer anyway would hide behind what next waited on it.

A declaration reports its layers (`also_provides`) and what they inject through
the methods `Discoverable` already has, so the 2026-10-31 entry's refusal of a
new descriptor field stands; the collect phase needs that report, to load a
config a layer injects, before anyone can tell which layers the pool will
register. Its reports name a layer a module may also list, so the access graph
reads a listing under a key's own type before them, and the seal holds a
declaration only to what the layers it registered inject, read off what it
attached: a layer the app holds some other way is never built from its `#[inject]`
fields, which for `ThrottlerGuard`, a factory's, name what no provider makes on
purpose. That holds in the access graph and the check of what a test's override
removed. A walk of the graph follows no declaration at all: one node holds every
layer it alone lists, so its report would read their dependencies as one
provider's and name a cycle none of them is on. `record_stall` refuses a cycle among
a pool's layers; one through a listed provider shows once the refusal's remedy, to
list the layer, is applied. A scoped layer is refused before those, as a layer, its
one remedy.

**The price:** a singleton provider of the root injecting a layer only a pool lists
is built before that layer exists, and is refused saying to list it
(`UnbuiltCause::RegisteredByAPool`), as NestJS's `APP_*` tokens are not injectable
by class; a request-scoped one, built after the pool, boots. **Refused:** registering a layer at the declaration's own step, where
"already provided" is a read of every reached descriptor that a hand-written
module's later registration contradicts, and where the step's static wait on the
layer's dependencies stalls on those of a layer something else provides.
