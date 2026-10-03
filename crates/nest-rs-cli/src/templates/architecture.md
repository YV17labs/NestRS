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

**A name and its path say the same thing, and that is the whole law**: from a
path you know the type, from a type the file, and a name in a stack trace
identifies itself. **The stem is the crate's subject plus every folder below
`src/`, joined.**

| path | type |
|---|---|
| `nest-rs-http/src/module.rs` | `HttpModule` |
| `nest-rs-throttler/src/module.rs` | `ThrottlerModule` |
| `nest-rs-redis/src/queue/module.rs` | `RedisQueueModule` |
| `nest-rs-redis/src/throttler/module.rs` | `RedisThrottlerModule` |
| `nest-rs-seaorm/src/database/module.rs` | `SeaOrmDatabaseModule` |
| `features/src/audio/http/module.rs` | `AudioHttpModule` |

**A port keeps the bare name; a driver carries its own**: `ThrottlerModule` is
the port's, `nest_rs::redis::RedisThrottlerModule` Redis's binding — unambiguous
in a log beats short in an import. An adapter crate is named for the vendor the
developer touches, the storage when the library is hidden (`nest-rs-redis`) or
the library when it is the surface (`nest-rs-seaorm`); never for a capability.

## Ports & Adapters — three module shapes, and no fourth

A **port** defines a contract *and the semantics that travel with it*; an
**adapter**, named for a vendor, carries only the transport and depends on the
port, which names no vendor. A library already multi-backend (sea-orm,
object_store) is wrapped, never abstracted.

| Shape | Example | Role | Variables |
|---|---|---|---|
| `<Vendor>Module::for_root(cfg)` | `SeaOrmModule`, `RedisModule` | opens the **resource** — the pool, the connection — once, for every binding in the crate | `<PREFIX>_<VENDOR>__*` |
| `<Port>Module::for_root(cfg)` | `ThrottlerModule`, `HttpModule`, `HealthModule` | the **capability**: its policy, its guard, its default implementation — when the port has any | `<PREFIX>_<PORT>__*` |
| `<Vendor><Port>Module` | `SeaOrmDatabaseModule`, `RedisQueueModule`, `RedisThrottlerModule` | **binds** the vendor to the port; a bare import, unless it owns settings of its own — then a `for_root` | `<PREFIX>_<VENDOR>__<PORT>__*` |

A module owning a `#[config]` offers a `for_root`; a bare import still reads its
variables. An adapter crate's root holds the resource and **one folder per port
it binds** (`queue/`, `throttler/`); a binding never names a sibling's type, and
a port crate with nothing to register holds no module. **Swapping a backend
edits the composition root alone**: a vendor binding supersedes the port's
default anywhere in `imports`, and two bindings of one port fail the boot.

**The crate counts only when it is a subject** (`features` is a container:
`audio/http/module.rs` is `AudioHttpModule`). **Every type in a `module.rs`
shares the stem** (`OAuthResourceModule`, `OAuthResourceSetup`,
`OAuthResourceHost`); a `#[module]` lives only in a `module.rs`, and another
provider gets a file named for it. `posts/http/controller.rs` is
`PostsController`; an edge folder right under a framework crate's `src/` adapts
the crate, and under a product crate's is refused. **The one precedence**: a
role file whose subject is a capability keeps its name (`TranscodeGuard`),
never a `module.rs`. Nothing below the root carries the project's or the app's
name. A module is plural for enumerable things (`users`), singular for a
capability (`auth`) — the generator singularizes it into the entity.

## Families — a shared prefix names a standard, never a theme

A prefix is a claim compiled into every path, target and variable the family
owns, so **a family exists only when one external standard names each member**,
and the next word is read off its vocabulary — RFC 6749 §1.1:

| Crate | Path a caller types | Read off |
|---|---|---|
| `nest-rs-oauth-client` | `nest_rs::oauth::client` | §1.1 *client* |
| `nest-rs-oauth-server` | `nest_rs::oauth::server` | §1.1 *authorization server* |
| `nest-rs-oauth-resource` | `nest_rs::oauth::resource` | §1.1 *resource server* |

Membership is a test an outsider can answer (*does the standard name this?*,
never *is this about auth?*); no crate carries the prefix alone, and a theme is
not a family — `authn` and `authz` are two crates, not two families.

## The product's own — what the product decides, and nothing else

A product module is named from its path like any other, with **no marker** to
tell it from a framework name. Where the two collide, the framework's is written
in full at the point of use, never aliased — `#[module]` names a module in boot
errors by its own ident: `#[module(imports =
[nest_rs::authn::AuthnModule::for_root(None)])] pub struct AuthnModule;`. **What
every consumer would write identically belongs to the framework** — a wire shape
a specification fixes, a type with no members declared only for a macro to gate
on. The product keeps what it decides: claims, policy, scopes, validation.

## Modules — two files, two jobs, never merged

| File | Job | Answers to | Holds |
|---|---|---|---|
| `mod.rs` | which **files** exist, and **what leaves the module** | the compiler, and readers | `mod`, `pub use` |
| `module.rs` | which **providers** exist, what is imported | the framework | exactly one `#[module]` |

`pub` means exported: what `mod.rs` re-exports is what the workspace may reach,
everything else is `pub(crate)` or private, and a `mod.rs` re-exporting
everything cancels the encapsulation. **No `*_module.rs`, ever**: one
`#[module]` per file, one `module.rs` per folder, two modules in two folders.

## Configuration — one seam per config, decided by ownership

| Whose config | Seam | In-code path |
|---|---|---|
| a **library** module's, in its own namespace | `Module::for_root(cfg)` | `for_root` — the base the env overlays, per field |
| **yours**, declared by your own module | `ConfigModule::for_feature::<C>()` | `impl Default` — you own the struct, so you edit it |
| nobody's (a discovered plugin) | its registry entry reads its own namespace | none — credentials are deployment data |

Who can edit the struct decides: `HttpModule::for_root(cfg)` sets a config you
cannot touch, your own config's `impl Default` is its in-code path, and it gets
a `for_root` only once an app must pin it from outside. The one homonym,
`ConfigModule::for_root()`, is the `.env` cascade, first in the root's imports.
**`for_root` configures; `for_feature` registers**, taking no value, so a
library module writes both: `for_feature` in `imports`, and `for_root(c: impl
Into<Option<C>>)` returning `ConfigModule::setup(c)` as a `ConfigSetup<M, C>`
alias (a hand-rolled `*Setup` only to queue more). **A module owning no config
gets no `for_root`** (`SocialModule` discovers providers that carry their own).

**A `#[config]`'s namespace is its stem, read and never chosen**, minus plural
role folders and a product's container: `redis/src/worker/config.rs` →
`<PREFIX>_REDIS__WORKER__*`, `social/src/providers/github/config.rs` →
`<PREFIX>_SOCIAL__GITHUB__*`, `features/src/oauth/config.rs` →
`<PREFIX>_OAUTH__*`; `<PREFIX>_DATABASE__URL` names neither crate nor type. One
namespace, one type; `Config` names a `#[config]` only (a nested struct is
vocabulary, `HttpTls`); seeding (`App::builder().provide`) is for hermetic
tests. A pinned base supersedes a bare import; two fail the boot
(`ContestedDeclarationError`), as does a config `App::new` cannot resolve.

## Providers — three questions, in order

`#[module]` takes only `imports` and `providers`; the name says what it is for:

1. **Is it listed in `providers`?** No ⇒ an entity, a `#[config]`, a DTO or
   vocabulary; only what is injected by type needs a module.
2. **Who calls it?** The framework, because of what it is ⇒ a **primitive**
   from the closed table below. Your code ⇒ custom.
3. **Does it own domain logic?** Yes ⇒ a **`Service`**. No ⇒ name it for what
   it is — factory, client, store, bridge, registry — never `Service`.

## Naming tables

File name = role, folder = module; snake_case; one role, one file per folder.

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

An adapter role carries its folder (`schedule/tasks.rs`, `mcp/guard.rs`), as
does a layer serving one transport; a crate whose subject is one edge keeps them
at its root (`nest-rs-server-timing`). **Custom providers** are named for what
they are, never folded into `service.rs`, a recognised word first.

**Vocabulary** (an enum, struct, alias or constants) is named for what it
declares, no role suffix: **the file and its folder, read together, spell the
type** — `seaorm/src/repo.rs` is `Repo`, `redis/src/connection.rs` is
`RedisConnection`, `authn/src/strategies/jwt.rs` is `JwtStrategy`. **`nestrs
lint` refuses one shape: a stem that appears nowhere in what the file
declares**, a file named for a slot rather than a subject. A word shared through
the folder (`throttler/store.rs` holds `RedisThrottler`), an inflection
(`scope.rs` holds `Scoped`), a namespace file (`consume::attempt`) and the
recognised provider words — `registry.rs`, `client.rs`, `store.rs`,
`factory.rs`, `source.rs`, `bridge.rs`, `inventory.rs` — pass. Vocabulary sits
flat at the module root, never in `types/` or `shared/`; a crowded root is a
module to split. No crate is `shared` or `common`; `core` is the one every
crate composes on and that composes on none. Test doubles are the one
crate-root file: `testing.rs`, `#[cfg(test)]`.

## Precedence — when a type carries a primitive role *and* logic

A primitive role wins only when the framework is the sole caller and the file
holds no domain logic: a hook, listener or tick never renames a service.

## Several of the same role

A plural folder carries several, one of a kind is a file (`dto.rs`), and the
singular trait file stays at the parent.

| Folder | File | Type |
|---|---|---|
| Providers — `services/`, `strategies/`, `pipes/` | bare: `input.rs` | `InputService` |
| Entities — `entities/` | bare: `user.rs` | `User` |
| Transfer objects — `dtos/`, `commands/` | suffixed: `login_dto.rs` | `LoginDto` |

A transfer object keeps its suffix, read far from its folder. Events take no
plural folder (`events/` is an edge): several sit at the port as
`<fact>_event.rs`. A second service comes only after a factory, client or enum.

## Folders

- **A file exists only with real content, and holds one subject**; wiring the
  root imports once is still `<name>/module.rs`. **A module's sub-folders are
  edge adapters and plural role folders, never a third kind** (`types/`).
- **The edge vocabulary is closed**: `http`, `graphql`, `ws`, `queue`,
  `schedule`, `mcp`, `events`; a new edge, `<edge>/module.rs` +
  `<Module><Edge>Module`, is a framework change.
- **A file under `<edge>/` serves that edge alone**; a type several edges
  dispatch to (a guard with `check_http` and `check_ws_message`) sits at the
  crate or module root, or the other edges need that edge's feature to reach it.
- `mod.rs` and `lib.rs` carry `mod`, `pub use` and, where the crate writes doc
  comments, a `//!` — no logic.
- An injected service field is `svc`, or `<name>_svc` beside others; other
  dependencies keep descriptive names (`db`, `queue`, `config`).

## Reserved vocabulary

A module may not take a name from the structural vocabulary — pick the domain
word (`programs`, not `apps`); `nestrs new` and `nestrs generate` refuse them.

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

A queue payload, a producer↔worker contract, lives at the port. The entity is
`Model` in `entity.rs`; its `#[expose]`d struct and `Create<E>`/`Update<E>` are
bare.
