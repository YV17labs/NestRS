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

## 2026-10-14 — the kernel and the config half landed; where they depart

**The inference compiles.** `ForInstance`'s rustdoc example boots the four
spellings inside `#[module(imports = [..])]` for a hand-written module generic
over `I`, a typed import, a generic call site and `provide_feature(None::<C>,
b)`. No fork went back to the owner.

**A named instance whose namespace is taken is refused as
`ConfigError::SharedInstance`, not `SharedNamespace`.** The two refusals carry
different remedies: two `#[config]`s sharing a namespace each take their own,
read off their paths, while an instance is renamed. One variant covers both
ways an instance's namespace is taken — a linked config declaring it (`Lock` on
a config whose binding declares `<vendor>__lock`), and another marker reading
the same name — since the variables under it could not say what they
configure either way. The refusal lives where the instance is first used, the
config read, so its tests are `nest-rs-config`'s; the twin markers sit in that
crate's integration suite beside the two configs sharing a namespace, read only
by the test that resolves them, so they need no binary of their own.

**`Unnamed::NAME` is empty.** The default inserts no segment, and its name is
the absence of one; `instance_namespace` reads the empty name and needs no
other case. `#[instance]` refuses `Unnamed`; `Primary` is an ordinary name.

**A bare import of an instance-capable module reads its instance unpinned
through `ForInstance::default()`**, the one way to write "nothing pinned" for
any `I`, since `None` converts for the default instance alone.

**The instance-aware read of a resolved config waits for its first caller.**
`ConfigModule::resolved::<C>` cannot take an instance without breaking every
turbofish call site: a function's type parameter has no default. A binding
reading `I::Of<C>` shapes it with the Redis bindings.

**`Instance` is sealed** behind a `__private` supertrait that `#[instance]`
implements, and core for `Unnamed`: an open trait let a hand-written impl
choose `NAME` and `Of`, reading a reserved word or holding a named config under
the default's key, unseen by the twin check — the *chosen name* refused below,
which only a seal holds, and sealing after 7.0 would be a break.

**A marker is told from its twins by its type, not its path.** Its entry
carries its `TypeId`: `module_path!()` leaves out a function a marker is
declared in, so a path compare refused a marker inside a test or a doctest as
its own twin.

**The default instance resolves through `Config::resolve`**, as every config
did before instances; a named one takes the same steps under its own
namespace. An override of `resolve` therefore reaches the default instance
alone; `Config::defaults` is the hook every instance reads.

**`#[instance]`'s reserved words are a list in `nest-rs-codegen`**, a
proc-macro being unable to read the rules file; a test in `nest-rs-cli` holds
it to the fence that crate already scrapes, so the two cannot drift.

**Status (2026-10-14):** the kernel (`Instance`, `Unnamed`, `Instanced`,
`#[instance]`, the generic-module refusal) and the config half (`ForInstance`,
`instance_namespace`, per-instance `provide_feature` and `setup`) landed with
`feat/named-instances-kernel`; Redis and the removal of string keys follow.

## 2026-10-17 — Redis is the first instance-capable adapter; where it departs

**The instance reaches the connection's type, all the way to a Cluster's
node.** `redis` opens each Cluster node through a static constructor
(`cluster_async::Connect`), with no value of ours in reach, so the line a node
files when it refuses the client's certificate can name its instance only
through a type. `RedisConnection<I>` therefore holds a link generic over `I`,
and a binding's store holding it is generic too — `RedisQueueProducer<I>`,
`RedisThrottler<I>`, `RedisOccurrenceLock<I>`, each defaulted, each built from
the connection it is handed, so `::new(conn)` infers it. Everything below the
Cluster's node — the link of one server, Sentinel's, the proofs and the
refusals — carries the instance as a value instead, read once at the open: a
helper generic over the marker would be compiled again per instance for no
caller. A store's default `ThrottlerStore::name` is its type name, and now
reads its instance (`RedisThrottler<Cache>`).

**`RedisConnection::connect` stays the default instance's.** A second public
constructor generic over the instance would make the bare call ambiguous
(`impl RedisConnection<Unnamed>` beside `impl<I> RedisConnection<I>`); a named
instance's connection is opened by `RedisModule::for_root((Cache, x))`, the one
seam, and seeded by its config (`Instanced<Cache, RedisConfig>`) in a test.

**A refusal holds its instance's variable names rendered** (`url_var`,
`connect_timeout_var`, … on `RedisError`), not a namespace to render later: a variant
reads the same wherever it is displayed, and the variant says which variable a
reader changes.

**The instance-aware read is `ConfigModule::resolved_for::<I, C>`**, beside
`resolved::<C>`, which keeps its turbofish callers and reads the default
instance through it. It hands back `Arc<I::Of<C>>`, borrowed as `C`, and its
refusal names the instance (`ConfigNotResolvedError::instance`).

**The unclaimed-variable report knows an instance's namespace from its
registration.** The kernel's entry traced it as needing no change; run, it
reported `<PREFIX>_REDIS__CACHE__CONNECT_TIMEOUT_SECS` under `redis` as a near
miss of `CONNECT_TIMEOUT_SECS`, since the default instance's keys are checked
when its read completes, before the cache's read. `ConfigModule::provide_feature`
now claims a named instance's namespace when a module registers it, before any
read, so it owns its variables from the first report, as `redis__queue` does
under `redis`. Refused: every linked marker crossed with every linked config —
one `#[instance] struct Cache` would then silence `seaorm__cache`, under a
vendor that opens no instance.

**Status (2026-10-17):** Redis — the connection, its three bindings and every
refusal and line — is instance-capable; storage keeps one disk (above), and
the removal of string keys follows.

## 2026-10-17 — string keys removed; where it departs

**`#[inject]` takes no argument, and `key` is answered, not unknown.** A 6.x
`#[inject(key = "…")]` is refused at the key with what replaced it — the
instance's type, or a newtype — through the grammar's slot for a key the
framework knows elsewhere; any other argument reads the shared sentence,
expecting none. The key is refused before its value is read, so a key that
was not a string earns the same sentence and no refusal of its own.

**The container keys on `TypeId` alone.** `ProviderKey` existed to carry a
key's name beside the type; with no name left, the singleton map, the access
graph's global set and the duplicate check all read one `TypeId`.

**An edge reads every field of the injectable body.** The keyed record went
unread because the edge decorators destructured the body with a rest pattern;
they now name each field, so a field the body grows cannot be dropped by one
edge in silence.

**Status (2026-10-17):** string keys are gone — `#[inject(key)]`,
`provide_keyed`, `get_keyed`, `ProviderKey`, `KeyedDependency` and its error;
storage keeps one disk (above).
