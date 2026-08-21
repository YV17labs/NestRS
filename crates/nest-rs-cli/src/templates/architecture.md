## Names — five levels, and none overflows into the next

| Level | Named for | Appears as |
|---|---|---|
| **Project** | the product | the repository and the workspace — **nowhere else** |
| **Family** | the **standard** that names its members | a shared crate-name prefix — and nothing else |
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
path you know the type; from a type you know where the file is. Nothing weaker is
worth having — a reader who has to *learn* which crate holds `SeaOrmDatabaseModule` has
lost the only property naming buys, and a name seen in a stack trace has to
identify itself without the path beside it.

**The stem is the crate's subject plus every folder below `src/`, joined.**

| path | type |
|---|---|
| `nest-rs-http/src/module.rs` | `HttpModule` |
| `nest-rs-throttler/src/module.rs` | `ThrottlerModule` |
| `nest-rs-redis/src/queue/module.rs` | `RedisQueueModule` |
| `nest-rs-redis/src/throttler/module.rs` | `RedisThrottlerModule` |
| `nest-rs-seaorm/src/database/module.rs` | `SeaOrmDatabaseModule` |
| `features/src/audio/http/module.rs` | `AudioHttpModule` |

**A port keeps the bare name; a driver carries its own.** `ThrottlerStore` is
the trait, and its implementations are already `InMemoryThrottler` and
`RedisThrottler` — the module follows the implementation, so
`nest_rs::redis::RedisThrottlerModule` sits beside `RedisThrottler` and says the
same thing. The bare `ThrottlerModule` belongs to `nest-rs-throttler`, which
defines the port. Two consequences, both deliberate: the path stutters
(`redis::RedisQueueModule`), and swapping a backend edits the type name as well
as the import. Both are paid on purpose — **a name that is unambiguous in a log
outranks a name that is short in an import**, and a module name appears in a
composition root, not in fifty call sites.

**A driver gives each port it binds a folder.** `nest-rs-redis` holds a queue, a
throttler and a worker, so it has `queue/`, `throttler/` and `worker/` — never a
`src/module.rs` whose own path fails to say what it is a module *of*.

**The crate counts only when it is a subject.** Every `nest-rs-*` is named for
what it holds, so it prefixes. A product library like `features` is a container
— its modules are domains, so `audio/http/module.rs` is `AudioHttpModule`, never
`FeaturesAudioHttpModule`.

**Every type in a `module.rs` shares the stem**, not just the module:
`RedisThrottlerModule`, `RedisThrottlerSetup`, `RedisThrottlerHost`. A rename
that leaves a sibling behind is half a rename, and the half left behind is the
one a reader trips on. The same reading gives the adapter's own types —
`posts/http/controller.rs` is `PostsController`, `users/ws/gateway.rs` is
`UsersGateway`.

Enforced, not merely written: `naming.rs` in `nest-rs-conformance` derives every
`module.rs` and every edge adapter in both workspaces and fails on a name that
does not match its path. Its baseline is empty and only shrinks.

**One documented precedence, and it is the only one.** A file whose subject is a
*capability* rather than its module keeps the capability's name — `audio`'s
`TranscodeGuard`, `posts`' `PostAuthorGuard`. That is the rule under
*Precedence* below, it is judgement rather than a scan, and it applies to role
files only: a `module.rs` never takes it.

**No module or provider below the root ever carries the project's or the app's
name.** The project name stops at the workspace; the app name stops at
`<App>Module`. An app may share the project's name only while it is the only
app — and even then, nothing beneath it may.

**`App` is the one exception, and it is not an app's name.** The apps are `api`,
`auth`, `live`, `worker`; `App` names none of them. It is the marker for *the
product's own* — the sense `AppAbility` and `AppJwtStrategy` already carry — and
the rule above forbids a **name**, never a marker. See *The product's own* below.

**A module name is plural when the domain is a collection of enumerable things
(`users`, `orders`), singular when it is a capability (`auth`, `search`).** Not
cosmetic: the generator singularizes the folder name to derive the entity, so a
wrongly pluralized module produces a wrongly named entity, silently.

## Families — a shared prefix names a standard, never a theme

`crates/` is one flat directory, read alphabetically. A shared prefix is
therefore the only grouping a reader is given for free, and it is worth having.
It is also a **claim**, compiled into every path, every span target and every
env var the family owns — so it has to be checkable, not merely helpful.

**A family exists when one external standard names each of its members.** The
prefix takes that standard's subject; the word after it is *read off* the
standard's own vocabulary rather than chosen. RFC 6749 §1.1 enumerates the roles
— *client*, *authorization server*, *resource server* — so the family is:

| Crate | Path a caller types | Read off |
|---|---|---|
| `nest-rs-oauth-client` | `nest_rs::oauth::client` | §1.1 *client* |
| `nest-rs-oauth-server` | `nest_rs::oauth::server` | §1.1 *authorization server* |
| `nest-rs-oauth-resource` | `nest_rs::oauth::resource` | §1.1 *resource server* |

**The membership test must be answerable by someone who did not write the code.**
*Does this standard name this thing?* is such a test. *Is this about auth?* is
not. A family whose membership is argued will be argued again, and every
re-argument renames crates — which is the cost the level exists to stop.

**No crate carries the prefix alone.** A name that is both a level and a member
denotes two things at once, and that breaks the property the whole model buys:
from a type you know the path. The family is `oauth`, so there is no
`nest-rs-oauth`.

**A theme is not a family.** Grouping *everything about auth* is a reading aid,
and reading aids belong in the documentation's own sections, which group without
asserting anything about what the code does. Alphabetical order already puts a
theme's members side by side; a prefix additionally states that the standard
above them is shared, which is a stronger claim and usually a false one.

**Recorded so it is not re-derived: `authn-*` / `authz-*` was considered, and the
standards retire it.** The instinct is sound — authentication and authorization
*are* the two halves of this territory — but the resulting family cannot be
tested. RFC 6749 titles itself *The OAuth 2.0 **Authorization** Framework*, so a
client and an authorization server are authz; social login is authentication
performed *through* that authorization flow; a JWT verifier serves both, because
one token carries the identity and the scopes; and RFC 9728 discovery is served
to callers who have no identity at all, so it is neither. One member in five
classifies without argument. `authn` and `authz` stay as **crate** names — the
pair is real, and it is the pair a reader wants — but they name two crates, not
two families.

## The product's own — `app_` where the name is already the framework's

A product module is named for its domain, and most domains are the product's
alone: `users`, `posts`, `audio`, `orders`. A few are not. Where the product
binds a framework concern, the obvious name is the concern's own word — and
then two modules wear it:

```rust
use nest_rs::authn::AuthnModule;      // the port
use features::authn::AuthnModule;     // the app's binding — same name
```

At the call site only `AuthnModule` is written, so the reader has to go back to
the `use` line to learn which one is imported. **That is the defect**: a name
that needs a second lookup is the one property the naming law exists to buy.

**So a product module whose name is one the umbrella re-exports takes the
`app_` prefix**, and every type in it follows:

| | |
|---|---|
| `features/app_authn/` | `AppAuthnModule`, `AppAuthnGuard`, `AppJwtStrategy` |
| `features/app_authz/` | `AppAuthzModule`, `AppAuthzHttpModule`, `AppAbility` |
| `features/app_oauth/` | `AppOAuthModule`, `AppOAuthHttpModule`, `AppOAuthService` |

Four things make it checkable rather than a matter of taste:

- **The set is closed and derived.** It is the `pub use nest_rs_* as <concern>;`
  list in `crates/nest-rs/src/lib.rs`, plus the members of each family module —
  one file, read by the conformance join. A module outside that set never takes
  the prefix: `users` and `posts` collide with nothing.
- **The marker goes in front**, because that is where the two ecosystems that
  have a convention put it — Rails' `ApplicationController` against
  `ActionController::Base`, and Rust's own `AppState` / `AppError` — and because
  the first word is the one a reader scans. NestJS needs none: `@nestjs/` in the
  import path does the same work a shared prefix does here.
- **The concern's word survives**, so the correspondence is immediate and
  `rg authn` still finds the binding. A suffix would keep that too; a rename
  (`identity`, `permissions`, `issuer`) would not.
- **It groups.** The bindings sort together above the product's own domains,
  which is the one grouping a flat directory gives away for free.

**The prefix is a claim about the module, so it binds what is inside it too.**
`app_authn/` holds `Claims` because `Claims` is what `JwtStrategy<Claims>`
binds; a separate `identity/` holding one file and no `module.rs` is that
content sitting where nothing reaches for it.

**And the claim is a test, so what is not the product's own does not live behind
the prefix.** Anything inside it that *every* consumer would write identically —
a wire shape a specification fixes, a token a framework seam requires, a default
nobody varies — is the framework's. Left in the app it is a copy waiting to
drift from the thing it duplicates, and the drift is silent because nothing
joins the two. **It moves up.** What earns the prefix is what this product
*decides*: its claims, its policy, its scopes, its validation rules.

Two tells, and both have been found here:

- **The fields are a specification's own field names.** `AccessTokenResponse`'s
  `access_token` / `token_type` / `expires_in` are RFC 6749 §5.1, so no
  conforming issuer can spell them otherwise. The framework already held §5.2's
  `TokenError` — one half of one response in the framework and the other half in
  the app is the asymmetry this rule names.
- **The type has no members at all.** A struct declared only so a macro has a
  concrete provider to gate on says nothing about the product; the seam that
  demands it should declare it.

This is *One declaration, every site the standard permits* read from the other
end: there the framework owes every site an answer, here the product owes the
framework anything that was never its own.

## Modules — two files, two jobs, never merged

| File | Job | Answers to | Holds |
|---|---|---|---|
| `mod.rs` | which **files** exist, and **what leaves the module** | the compiler, and readers | `//!`, `mod`, `pub use` |
| `module.rs` | which **providers** exist, what is imported | the framework | exactly one `#[module]` |

`module.rs` is the DI module. `mod.rs` is both the folder index *and* the
export contract: **its `pub use` list is what the rest of the workspace may
reach.** `pub` means exported; everything else is `pub(crate)` or private. A
`mod.rs` that re-exports everything cancels the encapsulation — that list is a
decision, not plumbing.

**No `*_module.rs`, ever.** One `#[module]` per file, one `module.rs` per
folder; two modules in a feature means two folders.

## Configuration — one seam per config, decided by ownership

A `#[config]` is reached through **exactly one** seam. Which one follows from
who owns it, and there is no judgement call:

| Whose config | Seam | In-code path |
|---|---|---|
| a **library** module's, in its own namespace | `Module::for_root(cfg)` | `for_root` — the base the env overlays, per field |
| **yours**, declared by your own module | `ConfigModule::for_feature::<C>()` | `impl Default` — you own the struct, so you edit it |
| nobody's (a discovered plugin) | its registry entry reads its own namespace | none — credentials are deployment data |

The split is *who can edit the struct*. You cannot touch `HttpConfig::default`,
so `HttpModule::for_root(cfg)` is the only way to set a port from code — which
is why the dual-path rule binds every `nest-rs-*` module. Your own
`IssuerConfig` needs no seam: its `impl Default` **is** the in-code path, and
adding a `for_root` nobody calls is speculative API in the exemplar people copy.
Write one the day an app needs to pin your config from outside your crate.

**`ConfigModule::for_root()` is the one homonym** — it takes no config and
configures no module. It switches on the `.env` cascade, and goes first in the
root's imports. Everything below is about `Module::for_root(x)`, which is a
different thing wearing the same name (NestJS's, kept deliberately).

**`for_root` configures; `for_feature` registers.** They are not two ways to do
one thing, and `for_feature` deliberately takes no value: a config reachable
through two seams is a config whose value depends on `imports = [..]` order.
A library module therefore writes **both** — `for_feature` in its `imports` so
the config always loads, and a `for_root` so a caller can pin it:

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

That is the whole seam when `for_root` only pins — reach for `ConfigSetup`
rather than hand-rolling a `*Setup`. Write your own only when `collect` queues
more than the config (a pool, a client) or `register` does more than recurse.

**The converse is load-bearing: a module that owns no config gets no
`for_root`.** *Owns* means its own namespace, never a config belonging to
something it merely discovers — `SocialModule` discovers providers that each
carry their own `#[config]`, so it stays a bare import with no seam at all.
Giving it one forces a list of unrelated config types, hence type erasure,
hence no duplicate detection.

**Pinning by seeding (`App::builder().provide(cfg)`) is not a seam** — a seed
short-circuits the resolving factory and freezes that whole namespace against
the deployment. It is the hermetic-test hatch, and nothing else.

You cannot get any of this wrong silently — the boot enforces it:

- a pinned base **supersedes** a bare import's env-only factory, wherever the
  two fall in `imports`;
- two pinned bases for one config **fail the boot** naming it
  (`ContestedDeclarationError`) — as do two modules binding the same
  implementation, which is how importing both throttler backends is caught;
- a config the synchronous `App::new` could never resolve **fails the boot**
  too (`UnresolvedFactoryError`), instead of surfacing as a `None` much later.

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
| Domain error / Static constants | `error.rs` / `constants.rs` |

An adapter role carries its folder: `schedule/tasks.rs`, never `tasks.rs` at
the module root. A transport-specific guard belongs to its adapter too
(`mcp/guard.rs`).

**The layer roles are on the table because they are dispatched to.** An
`#[interceptor]`, a `#[filter]` and an exception filter are mounted by the
framework exactly as a guard or a pipe is, so they are named for the role and
not for the type. The rule above binds them the same way: a layer that exists
to serve one transport sits in that transport's folder
(`http/interceptor.rs`), and a layer a module applies to itself whatever the
edge sits flat at the module root.

**A crate whose whole subject is one layer keeps it at the crate root.** There
is no `http/` folder to carry when the crate serves one edge and nothing else —
`nest-rs-server-timing`'s `interceptor.rs` is the whole crate, and wrapping it
in a folder would name an adapter the crate has no second of. The folder
separates *several* adapters within one module; it is not a suffix the role
carries everywhere.

**Custom providers.** Injectable, but nothing is dispatched *to* them. Named
for what they are, file named the same, and **never folded into `service.rs`**.
A recognised word beats an invented one: `Factory`, `Client`, `Store`,
`Registry`, `Source`, `Bridge`.

**Vocabulary.** Not registered anywhere: an enum, a struct, a type alias, a set
of constants. Named for *what it declares* — a role suffix on vocabulary is
noise. Shared test doubles are the one crate-root file: `testing.rs`, behind
`#[cfg(test)]`, doubles only.

## Precedence — when a type carries a primitive role *and* logic

A primitive role wins **only when the framework is the sole caller and the file
holds no domain logic**. `tasks.rs` earns its name when the clock is the only
caller and the work it drives lives in a service; otherwise it is a service
that happens to have a trigger. Same test for `#[hooks]`, `#[listeners]` and a
health indicator: a lifecycle hook or a scheduled tick never renames a service.

## Several of the same role

Pluralized sub-folder; the singular trait file stays at the parent.

| Folder | File | Type |
|---|---|---|
| Providers — `services/`, `strategies/`, `pipes/` | bare: `input.rs` | `InputService` |
| Entities — `entities/` | bare: `user.rs` | `User` |
| Transfer objects — `dtos/`, `commands/`, `events/` | suffixed: `login_dto.rs` | `LoginDto` |

A provider's role is spelled by its folder *and* its type, so the file does not
spell it a third time. A transfer object is read far from its folder — in a
handler signature — so it keeps the suffix at both sites.

**Two services in one module is a last resort.** Extracting a factory, a client
or an enum leaves the count at one, and that is the common case. Reach for
`services/` only when the module owns two bodies of domain logic.

## Folders

- A module that is not a feature still gets a folder — cross-cutting wiring
  imported once by the root is `<name>/module.rs`, never a top-level
  `<name>.rs`. A hand-written `impl Module` is still a DI module.
- **A module's sub-folders are a closed set of two kinds**: transport adapters
  and pluralized role folders (`services/`, `entities/`, `dtos/`, …). **There
  is no third kind.** A folder invented to group "things that go together" —
  `contract/`, `types/`, `core/`, `shared/`, `common/`, `interfaces/` — is a
  defect: every file it would hold is already named by a table above (a trait
  lives with its concern), so it sits flat beside
  its siblings. A folder that feels too full means the module is too big; split
  the module, never the vocabulary.
- **The edge vocabulary is closed**: `http`, `graphql`, `ws`, `queue`,
  `schedule`, `mcp`, `events`. The *form* is open — a new edge follows
  `<edge>/module.rs` + `<Module><Edge>Module` — but adding one is a framework
  change, not a local improvisation.
- `mod.rs` / `lib.rs` carry `//!`, `mod` and `pub use` — no logic.
- Injected service field is `svc` when there is one, `<name>_svc` when there
  are several. Non-service dependencies keep descriptive names (`db`, `queue`,
  `config`).

## Reserved vocabulary

**A module may not take a name from the structural vocabulary.** These words
already mean something to the layout, and reusing one makes every path
ambiguous. Pick the domain word instead — a module about desktop applications
is `programs`, not `apps`.

```
structure   apps  crates  features  src  tests
roles       mod  module  service  controller  resolver  gateway  tool
            processor  tasks  listener  guard  strategy  pipe  config
            interceptor  filter
            entity  error  constants  testing
plurals     services  entities  dtos  commands  events  strategies  pipes
edges       http  graphql  ws  queue  schedule  mcp  events
```

## Transfer objects — named for the boundary they cross

| Kind | Suffix |
|---|---|
| REST body, in or out | `Dto` — `LoginDto` |
| Queue payload, imperative ("do X", verb-led) | `Command` — `TranscodeCommand` |
| Queue payload, published fact (past tense) | `Event` — `OrderPlacedEvent` |
| WS message payload | `Dto` — `SendMessageDto` |
| GraphQL input, hand-written | `Input` |

A queue payload is a producer↔worker contract, so it lives at the port and the
processor imports it. The entity is the exception: it stays `Model` in
`entity.rs`, its `#[expose]`d wire struct keeps the bare entity name, and the
generated `Create<E>` / `Update<E>` are bare too.
