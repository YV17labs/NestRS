## Names — five levels, and none overflows into the next

| Level | Named for | Appears as |
|---|---|---|
| **Project** | the product | the repository and the workspace — **nowhere else** |
| **Family** | the **standard** that names its members | a shared crate-name prefix, and the first level of every path, span target and config namespace its members own — `nest_rs::oauth::client`, `<PREFIX>_OAUTH__CLIENT__*` |
| **Crate** | what it holds | its directory, and the root of every span target it emits |
| **App** | what it **serves** (`api`, `worker`, `auth`) | the binary, and `<App>Module` |
| **Module** | its **domain** (`users`, `billing`) | `<module>/`, `<Module>Module` |

```
<App>Module              the app's own module.rs     composition root
<Module>Module           <module>/module.rs           the port
<Module><Edge>Module     <module>/<edge>/module.rs    one adapter
<WhatItBinds>Module      <name>/module.rs             a substrate
```

**A name and its path say the same thing, and that is the whole law.** From a
path you know the type; from a type you know where the file is — and a name in
a stack trace identifies itself without the path beside it.

**The stem is the crate's subject plus every folder below `src/`, joined.**

| path | type |
|---|---|
| `nest-rs-http/src/module.rs` | `HttpModule` |
| `nest-rs-throttler/src/module.rs` | `ThrottlerModule` |
| `nest-rs-redis/src/queue/module.rs` | `RedisQueueModule` |
| `nest-rs-redis/src/throttler/module.rs` | `RedisThrottlerModule` |
| `nest-rs-seaorm/src/database/module.rs` | `SeaOrmDatabaseModule` |
| `features/src/audio/http/module.rs` | `AudioHttpModule` |

**A port keeps the bare name; a driver carries its own.** The bare
`ThrottlerModule` belongs to `nest-rs-throttler`, which defines the port and
its `InMemoryThrottler`; `nest_rs::redis::RedisThrottlerModule` sits beside
`RedisThrottler`. The path stutters and a backend swap edits the type name, on
purpose: **a name that is unambiguous in a log
outranks a name that is short in an import**.

**An adapter crate is named for the vendor whose types are the developer's
surface** — the storage when the library is hidden (`nest-rs-redis`: apalis is
an implementation detail), the library when it *is* the surface
(`nest-rs-seaorm`: entities, `Repo` and `DbErr` are sea-orm's). Never a
capability name worn by one backend: that is a port's word.

## Ports & Adapters — three module shapes, and no fourth

A **port** is a crate that defines a contract *and the semantics that travel
with it* — `nest-rs-queue` owns what a job attempt *is*. An **adapter** is named
for a vendor and carries *only the transport*: connect, fetch, acknowledge,
count. A library already multi-backend (sea-orm, object_store) is **wrapped,
never abstracted** — its URL scheme picks the backend. The adapter depends on
the port; a port names no vendor crate.

Every module in a composition root is one of three shapes:

| Shape | Example | Role | Variables |
|---|---|---|---|
| `<Vendor>Module::for_root(cfg)` | `SeaOrmModule`, `RedisModule` | opens the **resource** — the pool, the connection — once, for every binding in the crate | `<PREFIX>_<VENDOR>__*` |
| `<Port>Module::for_root(cfg)` | `ThrottlerModule`, `HttpModule`, `HealthModule` | the **capability**: its policy, its guard, its default implementation — when the port has any | `<PREFIX>_<PORT>__*` |
| `<Vendor><Port>Module` | `SeaOrmDatabaseModule`, `RedisQueueModule`, `RedisThrottlerModule` | **binds** the vendor to the port; a bare import, unless it owns settings of its own — then a `for_root` | `<PREFIX>_<VENDOR>__<PORT>__*` |

```rust
SeaOrmModule::for_root(None),      SeaOrmDatabaseModule,   SeaOrmHealthModule,
RedisModule::for_root(None),       RedisQueueModule,       RedisWorkerModule::for_root(None),
ThrottlerModule::for_root(None),   RedisThrottlerModule,
HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() }),
```

"Bare or `for_root`" is *Configuration*'s rule, not a fourth shape: a module
that owns a `#[config]` offers a `for_root`, and a bare import of it still reads
its variables — `for_root` is the sign of settings, never their precondition.

**The layout follows.** An adapter crate's root holds the resource
(`config.rs`, `connection.rs`, its own `module.rs`) and **one folder per port
it binds** (`queue/`, `throttler/` under `nest-rs-redis`), each with its
adapter types and `module.rs`. What several bindings share sits at the root
**and only that**, and a binding never names a sibling's type. A port crate
with nothing to register holds no module (`nest-rs-queue`): a second queue
adapter calls `nest_rs_queue::consume` and writes its fetch loop, nothing more.

**Swapping or adding a backend edits the composition root and nothing else.**
A vendor binding supersedes the port's default wherever it sits in `imports`;
two adapters binding one port fail the boot with `ContestedDeclarationError`,
naming both imports.

**The crate counts only when it is a subject.** A product library like
`features` is a container, so `audio/http/module.rs` is `AudioHttpModule`,
never `FeaturesAudioHttpModule`.

**Every type in a `module.rs` shares the stem**, not just the module:
`OAuthResourceModule`, the `OAuthResourceSetup` its `for_root` returns, the
private `OAuthResourceHost` carrying the `#[module]`. A provider that is not the
module's own — a lifecycle hook, an endpoint — gets a file named for it, and a
`#[module]` or `impl Module` lives in a `module.rs` and nowhere else. A rename
that leaves a sibling behind is half a rename.

Adapters read the same way: `posts/http/controller.rs` is `PostsController`,
and an edge folder directly under a framework crate's `src/` adapts the crate
(`nest-rs-x/src/http/controller.rs` is `XController`). **In a product crate
that folder is refused** — no name it could take is allowed — so a product's
edge adapter lives in `<module>/<edge>/`.

**One documented precedence, and it is the only one:** a role file whose
subject is a *capability* rather than its module keeps the capability's name —
`audio`'s `TranscodeGuard`, `posts`' `PostAuthorGuard`. A `module.rs` never
takes it.

**No module or provider below the root carries the project's or the app's
name**: the project name stops at the workspace, the app name at
`<App>Module` — even when the only app shares the project's name.

**A module name is plural for a collection of enumerable things (`users`),
singular for a capability (`auth`, `search`)** — the generator singularizes the
folder to derive the entity, so a wrong plural names the entity wrong.

`nestrs generate` writes the names a path derives, `nestrs lint` checks the one
it cannot (*Vocabulary*, below), and review holds the rest.

## Families — a shared prefix names a standard, never a theme

A shared prefix is a **claim**, compiled into every path, span target and env
var the family owns, so it has to be checkable. **A family exists when one
external standard names each of its members**: the prefix takes the standard's
subject, and the word after it is *read off* its vocabulary — RFC 6749 §1.1:

| Crate | Path a caller types | Read off |
|---|---|---|
| `nest-rs-oauth-client` | `nest_rs::oauth::client` | §1.1 *client* |
| `nest-rs-oauth-server` | `nest_rs::oauth::server` | §1.1 *authorization server* |
| `nest-rs-oauth-resource` | `nest_rs::oauth::resource` | §1.1 *resource server* |

**The membership test is answerable by someone who did not write the code** —
*does this standard name this thing?*, never *is this about auth?* — because
every re-argument of membership renames crates.

**No crate carries the prefix alone** — a name that is both a level and a
member denotes two things, so there is no `nest-rs-oauth`. **A theme is not a
family**: grouping *everything about auth* is a reading aid, which belongs in
the documentation. That is why `authn` and `authz` are two crates and not two
families — no standard sorts social login, a JWT verifier or RFC 9728
discovery into one or the other.

## The product's own — what the product decides, and nothing else

A product module is named from its path like any other — `features/authn/module.rs`
is `AuthnModule`, whether or not the framework exports that name. **No marker
is ever added to tell a product name from a framework one**: the path a caller
types separates them. A driver's `Redis` is a *subject* that names a backend; a
product marker such as `App` names nothing and buys only length.

**Where the product's name equals the framework's, the framework's is written
in full at the point of use** — not aliased (`#[module]` names a module in boot
errors by the struct's own ident, which an alias does not change):

```rust
// features/src/authn/module.rs — the product's AuthnModule binds the framework's
#[module(imports = [nest_rs::authn::AuthnModule::for_root(None)])]
pub struct AuthnModule;
```

**What is not the product's own does not live in the product.** What every
consumer would write identically — a wire shape a specification fixes, a token
a framework seam requires, a default nobody varies — belongs in the framework;
left in the app it drifts from what it duplicates. What stays is what the
product *decides*: its claims, policy, scopes and validation rules. Two tells:

- **The fields are a specification's own** — `access_token` / `expires_in` are
  RFC 6749 §5.1, and no conforming issuer spells them otherwise.
- **The type has no members.** A struct declared only so a macro has a provider
  to gate on says nothing about the product; the seam that demands it declares it.

## Modules — two files, two jobs, never merged

| File | Job | Answers to | Holds |
|---|---|---|---|
| `mod.rs` | which **files** exist, and **what leaves the module** | the compiler, and readers | `mod`, `pub use` |
| `module.rs` | which **providers** exist, what is imported | the framework | exactly one `#[module]` |

`module.rs` is the DI module; `mod.rs` is the folder index *and* the export
contract: **its `pub use` list is what the rest of the workspace may reach.**
`pub` means exported; everything else is `pub(crate)` or private, and a
`mod.rs` re-exporting everything cancels the encapsulation.

**No `*_module.rs`, ever.** One `#[module]` per file, one `module.rs` per
folder; two modules in a feature means two folders.

## Configuration — one seam per config, decided by ownership

A `#[config]` is reached through **exactly one** seam, and who owns it decides
which:

| Whose config | Seam | In-code path |
|---|---|---|
| a **library** module's, in its own namespace | `Module::for_root(cfg)` | `for_root` — the base the env overlays, per field |
| **yours**, declared by your own module | `ConfigModule::for_feature::<C>()` | `impl Default` — you own the struct, so you edit it |
| nobody's (a discovered plugin) | its registry entry reads its own namespace | none — credentials are deployment data |

The split is *who can edit the struct*. You cannot touch `HttpConfig::default`,
so `HttpModule::for_root(cfg)` is how a port is set from code, and every
`nest-rs-*` module offers both paths. Your own config's `impl Default` **is**
its in-code path; write a `for_root` the day an app must pin it from outside
your crate, not before.

**`ConfigModule::for_root()` is the one homonym**: it configures no module,
switches on the `.env` cascade, and goes first in the root's imports.

**`for_root` configures; `for_feature` registers** and takes no value, since a
config reachable through two seams depends on `imports` order. A library module
writes **both** — `for_feature` so the config always loads, `for_root` so a
caller can pin it:

```rust
#[module(imports = [ConfigModule::for_feature::<StorageConfig>()], providers = [Storage])]
pub struct StorageModule;

impl StorageModule {
    pub fn for_root(config: impl Into<Option<StorageConfig>>) -> StorageSetup {
        ConfigModule::setup(config)
    }
}

pub type StorageSetup = ConfigSetup<StorageModule, StorageConfig>;
```

That is the whole seam when `for_root` only pins; hand-roll a `*Setup` only
when it queues more than the config (a pool, a client).

**A module that owns no config gets no `for_root`.** *Owns* means its own
namespace: `SocialModule` discovers providers that each carry a `#[config]`, so
it stays a bare import — a seam there would erase their types and lose
duplicate detection.

**A `#[config]`'s namespace is its stem, exactly as its type name is — read,
never chosen.** The segments are the crate's subject, then every folder below
`src/` that is not a pluralised role folder, joined by `__`:
`redis/src/worker/config.rs` → `RedisWorkerConfig` → `<PREFIX>_REDIS__WORKER__*`;
`oauth-client/src/config.rs` → `<PREFIX>_OAUTH__CLIENT__*`, the same string as
`nest_rs::oauth::client`; `social/src/providers/github/config.rs` →
`<PREFIX>_SOCIAL__GITHUB__*`; and a product crate leaves its container out, so
`features/src/oauth/config.rs` → `OAuthConfig` → `<PREFIX>_OAUTH__*`.
`<PREFIX>_DATABASE__URL`, the universal convention, names neither the crate nor
the type that parses it, and is exactly what this forbids. **One namespace, one
type**: two `#[config]` on one namespace fail the boot; two configs, two folders.

**`Config` names a `#[config]`, and nothing else.** A settings struct a config
nests is vocabulary: `http/src/tls.rs` holds `HttpTls`, not an `HttpTlsConfig`
that reads as a namespace which does not exist. **Seeding**
(`App::builder().provide(cfg)`) is not a seam: it freezes the namespace against
the deployment, and is the hermetic-test hatch only.

The boot holds the rest: a pinned base supersedes a bare import's env-only
factory wherever it sits in `imports`; two pinned bases for one config fail
with `ContestedDeclarationError`; a config the synchronous `App::new` could
never resolve fails with `UnresolvedFactoryError` rather than a late `None`.

## Providers — three questions, in order

`#[module]` takes only `imports` and `providers`. There is no `controllers`
list, so the *mechanism* cannot say what a thing is for. **The name has to.**
Answer these before naming anything.

**Q1 — is it listed in `providers`?** No ⇒ it is not a provider. It is either
something the framework *consumes* without injecting (an entity, a `#[config]`,
a DTO the validator reads) or plain vocabulary (an enum, a type alias, a set of
constants). Both are named by the tables below; neither needs a module of its
own. **Only something injected by type needs a module.**

**Q2 — who calls it?** The framework, *because of what it is* ⇒ **primitive**:
the vocabulary is closed, you pick from it rather than invent. Your own code
⇒ **custom** ⇒ Q3.

**Q3 — does it own domain logic?** Yes ⇒ it is a **`Service`**, and that is the
residue by design. No ⇒ name it for **what it is** — a factory, a client, a
store, a bridge, a registry, a transport seam — and never `Service`.

## Naming tables

File name = role, folder = module. Snake_case, no dotted variants, **one role
→ one file per folder**.

**Mounted or injected primitives.** The framework dispatches to these; the file
is named for the role, never for the type.

| Role | File |
|---|---|
| DI module (exactly one `#[module]` struct per file) | `module.rs` |
| Folder index (`pub use` / `mod` only) | `mod.rs` |
| Service | `service.rs` / `services/` |
| Controller (REST) / Resolver (GraphQL) / Gateway (WS) | `http/controller.rs` / `graphql/resolver.rs` / `ws/gateway.rs` |
| Processor (queue) / Scheduled tasks / Tool (MCP) | `queue/processor.rs` / `schedule/tasks.rs` / `mcp/tool.rs` |
| Event listener host | `events/listener.rs` |
| Entity (ORM + `#[expose]`) | `entity.rs` / `entities/` |
| Guard / Strategy / Pipe | `guard.rs` / `strategy.rs` / `pipe.rs` |
| Interceptor / Filter / Exception filter | `interceptor.rs` / `filter.rs` / `exception_filter.rs` |
| Module config (`#[config]`) | `config.rs` |
| Error types (every one — public, crate-private, domain or driver defect; a wire error document a client parses, such as `ProblemDetails`, is named for its document) / Static constants | `error.rs` / `constants.rs` |

An adapter role carries its folder (`schedule/tasks.rs`, `mcp/guard.rs`). A
layer serving one transport sits in its folder (`http/interceptor.rs`); one a
module applies whatever the edge sits at the module root. **A crate whose whole
subject is one edge keeps its role files at the crate root**
(`nest-rs-server-timing`'s `interceptor.rs`): the folder separates *several*
adapters, and there are none to separate.

**Custom providers** are injected but never dispatched to: named for what they
are, file named the same, **never folded into `service.rs`**, and a recognised
word beats an invented one — `Factory`, `Client`, `Store`, `Registry`,
`Source`, `Bridge`.

**Vocabulary.** Not registered anywhere: an enum, a struct, a type alias, a set
of constants. Named for *what it declares* — a role suffix on vocabulary is
noise. **The file and its folder, read together, spell the type**: one of the
two names the *kind*, never both and never neither.

- **The kind is the subject** — `seaorm/src/repo.rs` is `Repo`.
- **The file names the kind**, the type prepends the subject —
  `redis/src/connection.rs` is `RedisConnection`, `events/src/bus.rs` is
  `EventBus`.
- **The folder names the kind**, the file names the subject —
  `authn/src/strategies/jwt.rs` is `JwtStrategy`.

**One shape is refused, and only one: a stem that appears nowhere in what the
file declares.** Ask: **does either name reach the other?** If neither does, the
file was named for a *slot* — "who acts", "what we pass around" — not a
subject; a slot has no admission test, so it fills. It is `shared/` at the
scale of a file, and only the pair reads wrong.

Everything short of that passes, deliberately — a tighter test is false on a
third of the framework:

- **The shared word may come from the folder**: `throttler/store.rs` holds
  `RedisThrottler` — the file names the *seam*, the type *what fills it*.
- **An inflection is the same word**, and so is a word in the middle:
  `scope.rs` holds `Scoped`, `token.rs` holds `AccessTokenRequest`.
- **A recognised word is a role, not vocabulary.** `registry.rs`, `client.rs`,
  `store.rs`, `factory.rs`, `source.rs`, `bridge.rs` and `inventory.rs` take the
  custom-provider pairing (`<Subject>Registry`).
- **A file whose principal export is not a type is a namespace** and owes
  nothing: `queue/src/consume.rs` exports `consume::attempt`, and the call site
  reads the stem as part of the name.

`nestrs lint` runs this pairing over a project's `src/` and refuses only the
one shape; files a table already names are skipped.

**Vocabulary sits flat at the module root.** It is never gathered into
`types/`, `model/`, `common/` or `shared/`: a folder named for who uses it has
no admission test, so nothing can be refused from it and it fills. A crowded
module root is a module to split, not vocabulary to bury.

**The same rules out a `shared` or `common` crate**: they name *who reaches
for* it, so they admit anything. If a crate's subject cannot be named in one
noun, its vocabulary belongs to the module that owns it. **`core` survives on a
test** — every other crate composes on it and it composes on none of them; a
`core` that fails it is a `shared` wearing a better word.

Shared test doubles are the one crate-root file: `testing.rs`, behind
`#[cfg(test)]`, doubles only.

## Precedence — when a type carries a primitive role *and* logic

A primitive role wins **only when the framework is the sole caller and the file
holds no domain logic**: `tasks.rs` earns its name when the work it drives
lives in a service. Same test for `#[hooks]`, `#[listeners]` and a health
indicator — a hook or a tick never renames a service.

## Several of the same role

Pluralized sub-folder; the singular trait file stays at the parent. **The
folder exists to carry *several*, so one of a kind is a file** — a single
transfer object is `dto.rs` at the module root.

| Folder | File | Type |
|---|---|---|
| Providers — `services/`, `strategies/`, `pipes/` | bare: `input.rs` | `InputService` |
| Entities — `entities/` | bare: `user.rs` | `User` |
| Transfer objects — `dtos/`, `commands/` | suffixed: `login_dto.rs` | `LoginDto` |

A provider's role is spelled by its folder *and* its type, so the file does not
spell it a third time. A transfer object is read far from its folder — in a
handler signature — so it keeps the suffix at both sites.

**Events take no plural folder, because `events/` is an edge.** One event
payload is `event.rs`; several sit flat at the port as `<fact>_event.rs`
(`post_published_event.rs`), and `events/` holds the listener host alone.

**Two services in one module is a last resort** — extract a factory, a client
or an enum first; `services/` is for two bodies of domain logic.

## Folders

- **One file, one subject.** A subject that needs *and* to state is two files
  — a procedure and the record it writes — however short.
- Cross-cutting wiring imported once by the root is still a module in a
  folder, `<name>/module.rs`, never a top-level `<name>.rs`.
- **A module's sub-folders are a closed set of two kinds**: transport adapters
  and pluralized role folders (`services/`, `entities/`, `dtos/`, …). **There
  is no third kind.** A folder invented to group "things that go together" —
  `contract/`, `types/`, `core/`, `shared/`, `common/`, `interfaces/` — is a
  defect: every file it would hold is named by a table above, and a trait lives
  with its concern. A folder that feels too full means the module is too big.
- **The edge vocabulary is closed**: `http`, `graphql`, `ws`, `queue`,
  `schedule`, `mcp`, `events`. The *form* is open — a new edge follows
  `<edge>/module.rs` + `<Module><Edge>Module` — but adding one is a framework
  change, not a local improvisation.
- **A file under `<edge>/` serves that edge and nothing else.** A type the
  framework dispatches to at several edges — a guard implementing `check_http`
  and `check_ws_message`, a layer bound at more than one edge — sits **at the
  level every edge it answers can reach**: the crate root, or the module root
  beside the edge folders — or every other edge must enable that edge's
  feature to reach it. Only what dispatches to the type shows this.
- `mod.rs` / `lib.rs` carry `mod` and `pub use`, and a `//!` where the crate
  writes doc comments — no logic.
- Injected service field is `svc` when there is one, `<name>_svc` when there
  are several. Non-service dependencies keep descriptive names (`db`, `queue`,
  `config`).

## Reserved vocabulary

**A module may not take a name from the structural vocabulary.** These words
already mean something to the layout, and reusing one makes every path
ambiguous. Pick the domain word instead — a module about desktop applications
is `programs`, not `apps`. `nestrs new` and `nestrs generate` refuse them.

```
structure   apps  crates  features  src  tests
roles       mod  module  service  controller  resolver  gateway  tool
            processor  tasks  listener  guard  strategy  pipe  config
            interceptor  filter
            entity  error  constants  testing
singulars   dto  command  event
plurals     services  entities  dtos  commands  strategies  pipes
edges       http  graphql  ws  queue  schedule  mcp  events
```

## Transfer objects — named for the boundary they cross

| Kind | Suffix |
|---|---|
| REST body, in or out | `Dto` — `LoginDto` |
| Queue payload, imperative ("do X", verb-led) | `Command` — `TranscodeCommand` |
| Published fact, on the event bus or a queue (past tense) | `Event` — `OrderPlacedEvent` |
| WS message payload | `Dto` — `SendMessageDto` |
| GraphQL input, hand-written | `Input` |

A queue payload is a producer↔worker contract, so it lives at the port and the
processor imports it. The entity is the exception: it stays `Model` in
`entity.rs`, its `#[expose]`d wire struct keeps the bare entity name, and the
generated `Create<E>` / `Update<E>` are bare too.
