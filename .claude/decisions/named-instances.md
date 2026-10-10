# A resource opened twice is told apart by a type: the instance marker

An app that queues and caches needs two Valkeys: the queue refuses `allkeys-*`
eviction (`queue-redis-streams.md`) and a cache lives by it. No module can open
a second connection: a resource is identified by its `TypeId`, two imports of
one configured module yield one instance, and string-keyed injection — the one
way to hold two values of a type — is refused wherever something injects it
(below). The
owner decided on 2026-10-09: named instances, told apart by a typed marker,
never a database.

**An instance is a marker type**, declared `#[instance] pub struct Cache;`. The
decorator writes the `Instance` impl, and the instance's name is the ident in
snake case (`ReadReplica` → `read_replica`), read and never chosen, as a
config's namespace is its stem. The default instance is `Unnamed`: it adds no
segment, so an app with one Redis compiles and reads its variables as before.
`RedisConnection<Cache>` is a type of its own, so its budget, its net, a
contested declaration and an injection are per instance with no kernel change;
the container holds a named instance's config as `Instanced<Cache, RedisConfig>`
and the default's as the config itself, so a seed of `RedisConfig` still works.

**A value names the instance at `for_root`; a type argument names it at a typed
import**: `RedisModule::for_root(None)` and `RedisQueueModule` are the default
instance, `RedisModule::for_root((Cache, None))` and
`RedisThrottlerModule::<Cache>` the `cache` one. Rust applies no default type
parameter during expression inference: with `impl<I: Instance> RedisModule<I>`,
a bare `RedisModule::for_root(None)` leaves `I` uninferred, and a second
inherent `for_root` on the default is ambiguous. So the instance travels in the
value, by `container.md`'s law — `x` grows a field, never the seam a method —
as `ForInstance<I, C>`, a third shape of `x` for a module that can be opened
twice: the default takes `x`, a named instance `(Marker, x)`. A type position
does apply defaults, so a typed import keeps the turbofish.

That inference is reasoned, not yet compiled. The kernel's first test compiles
the four spellings inside `#[module(imports = [..])]` and a generic call site;
if one fails to infer, the fork goes back to the owner before any adapter is
written: the turbofish, the default keeping its bare spelling through a type
alias if one works — never a second constructor.

**Namespaces read vendor, instance, port** — `<PREFIX>_REDIS__CACHE__URL`,
`<PREFIX>_REDIS__CACHE__QUEUE__*` — one order for two crossed axes
(`naming.md`). `#[instance]` refuses a reserved word at expansion, so no
instance is named `Queue`. An instance whose derived namespace equals a linked
config's that the word list cannot know — `Lock` on Redis reads `redis__lock`,
which a package binding Redis to its own lock port may declare — is refused as
a shared namespace, and two markers reading one name are refused at first use.
Every refusal names its instance's variables, and every line a connection
files carries an `instance` field.

**A module generic over an instance is written by hand.** `#[module]` submits a
`static` descriptor, which cannot be generic, so it refuses a generic struct and
names the hand-written form, `impl<I: Instance> Module for RedisQueueModule<I>`
— the status the Redis bindings and `SeaOrmDatabaseModule` already have.

**The set** (`naming.md`): `Instance`, `Unnamed`, `#[instance]`,
`Instanced<I, T>`, `ForInstance<I, C>`, read off the domain word *instance*
(NestJS's named connections, Laravel's connections, Spring's qualifiers); the
next member binding a value to an instance is `Instance…` or `…Instance`. Held
by the types, `#[instance]`'s expansion checks and the shared-namespace refusal.

**Storage keeps one disk in 7.0.** A hand-written `StorageModule<I>` registers
`Storage<I>` imperatively, outside the access graph: any module could inject
it, where `Storage` is today a listed provider only an importer reaches. Named
disks wait for a seam that puts hand-written generic modules back in the access
graph. The default disk keeps its spelling — a defaulted type parameter and a
widened `for_root` are both additive — so they arrive in a minor. Until then the
family's rule stands for every adapter: a resource that may be opened twice is
generic over `I: Instance`, and every refusal and line names its instance.

**Refused:**

- **String keys** (`#[inject(key = "cache")]`, `provide_keyed`), removed in
  7.0. As shipped they could not work: `App::new` seals with an empty keyed set,
  so a reachable keyed dependency fails the boot; a keyed value a module
  installs in `register` is refused as soon as anything injects it, and no
  factory form installs one; an eager consumer resolves through `.expect` and
  panics before the typed error. Repaired, they would be two mechanisms for one
  concept, one of them unchecked by rustc. Two values of one foreign type are
  two types: a newtype, or `Instanced<Marker, T>`.
- **A turbofish at `for_root`** (`RedisModule::<Cache>::for_root(None)`): the
  default instance loses its bare spelling (`RedisModule::<Unnamed>::for_root`
  in every app) or gains a second constructor, a hard "no".
- **A builder method on the setup** (`.instance(Cache)`): a second seam where
  `container.md` allows no inherent method on a `*Setup`.
- **A chosen name** — a hand-written `impl Instance` with its own `NAME`: the
  segment would be chosen, not read, and the trait's associated type is the
  decorator's to write.
- **A `const` assertion on reserved names**: it fails after monomorphization,
  which `cargo check` does not report in generic code. `#[instance]`'s word list
  and the shared-namespace refusal hold it.
- **`Primary` as the default's name**: in `nest-rs-redis` "primary" is the
  Sentinel node role (`RedisError::PrimaryUnknown`), so
  `RedisConnection<Primary>` would read as the connection to the primary node.
  `Default` would shadow the trait.
- **`<PREFIX>_REDIS_CACHE__*`, or the instance after the port**: one order.
- **Instances of `SeaOrmModule`**: `Repo` holds one connection per binary
  (`modular-monolith-per-workload.md`); a second database is a second project.
- **Named storage disks bought with storage's import gating.**

**Status (2026-10-10): decided, not landed.** The kernel and the config half
land with `feat/named-instances-kernel`, Redis with `feat/redis-named-instances`,
the removal of string keys with `refactor/keyed-injection-out`; each appends an
entry here where it departs from this one.
