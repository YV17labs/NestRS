---
paths:
  - "crates/nest-rs-*/**/*.rs"
  - "crates/nest-rs-*/**/*.toml"
---

# Framework crates — macros, container, discovery

Loaded when touching `crates/nest-rs-*`. See also: `request-layers.md`,
`data-layer.md`.

## Macros

**Reach for macros first.** When wiring a service, module or endpoint,
use the decorators. When a pattern recurs without one, write a new
decorator — if it clears the bar below.

**A refusal lands at the earliest site that can see the fact.** When a
declaration is wrong in a way something can *know*, the question is only
who knows it first — and that site owes the error, because every site
after it costs the developer a run, a boot, or a silence. The
provider-hosted decorators are the worked example, one fact
(`Container::get::<Host>()` answers only for a singleton under its own
type) refused at four different sites:

| Knowable at | Shape | Answer |
|---|---|---|
| the host's own decorator (scope is right there) | `scope = request`, `scope = transient` | compile error, reading `ProviderResidency::SINGLETON` |
| the impl half's expansion | an edge host — metadata only, no instance | the same compile error |
| the boot, from this app's composition | held under another key — `dyn Trait`, a `for_root`, a hand-written `Module` | `warn` + `INERT_HOST_HINT` |
| the boot, from this app's imports | module not imported | the same `warn` |

**Stated, never merely absent.** A refusal that reads a *missing* marker is
fillable: `ProviderResidency` was a bare `Singleton` trait for one audit round,
and `impl Singleton for PerResolution {}` — the line its own note recommended to
hand-written providers — put a `scope = transient` host back through the bound
silently. Every decorator that builds a provider now writes the fact, `true` or
`false`, so contradicting it is `E0119` and the hatch survives only where nothing
has spoken. Testable form: a trybuild snapshot per refused shape *plus* one that
tries the escape.

**A value refusal opens with its site.** Every refusal of a value a decorator
reads — a key's value, a positional argument, an element of a list — opens with
the decorator and the position, worded once in `nest_rs_codegen::site`: the key
(``#[process] `retries` takes a whole number``), the word the grammar names a
positional by (``#[redirect] `status` ``), or the decorator alone for its one
positional (`#[every("30s")]`, `#[get]`). The site is the part a problems list or
a CI summary shows without the source frame, and `transactional` is a key of four
decorators. Three shapes follow it, one per mistake: a value of the wrong kind
reads ``… takes <what>`` (`takes_value`, `require_str_lit`), one sentence per
position whatever was written instead; a value that breaks a grammar names itself
and the rule (``#[controller] `version`: "1/2" is not a path segment — …``); and a
value outside a closed set keeps `unknown_value`'s ``unknown #[attr] <what> `x`;
expected …``, which names the decorator as well. No decorator hands a value to
`syn`'s own sentence, which names neither the decorator nor the key, nor to
`format_ident!`, which panics: a parse failure is re-worded at the token `syn`
stopped on. A value read out of another crate's attribute opens with the site
where it is written (``#[sea_orm] `from` ``, as `#[expose]` reads it), and every
value reader reads through the invisible group a `macro_rules!` forwards a value
in (`ungrouped_expr`). A `*-macros` crate words its value refusals through
`takes_value` like `nest_rs_codegen` does, with a trybuild snapshot per refusal.

**A rule both a decorator and the runtime check is written twice and pinned
once.** A `proc-macro` crate exports only macros and a surface crate cannot depend
on its own macros' crate, so a queue name checked on a `#[queue]` literal at
compile time and on a runtime name at the push is two functions —
`nest_rs_codegen::is_valid_queue_name` and `QueueName::is_valid` — and that is the
one duplication allowed. It owes three things: **one** test in the surface crate
running both over one corpus, its bounds read from the runtime's constants
(`QueueName::MAX_LEN`) rather than retyped; **one fact** in both sentences — the
compile error is the runtime's with the site in front, so the two give the same
reason; and a `pub mod` in `nest_rs_codegen` only when the runtime needs a *name*
from it, as `versioning` does — a function the test calls is re-exported flat.

**A `warn` may name causes; it may not prescribe an edit the framework cannot
verify.** The same hint offered "list it in `providers` under its own type as
well" — and `providers = [Foo, Foo as dyn Trait]` runs the constructor twice,
so the decorators fire on one instance while every `Arc<dyn Trait>` consumer
holds another, with nothing to notice; on a hand-written `impl Module` the same
edit fails the boot. Five causes reach that one skip line and the container
cannot tell which, so it names them and stops.

**Escalate no further than the fact supports.** The last two rows are correct
in another composition, so they warn — a boot error there would refuse working
code. And **a `warn` whose sentence is wrong is worse than none**: that line
claimed *unreachable from app's module tree* about a provider written in
`providers`, sending the reader to check the one thing already true. One shared
sentence, every site, or the wording drifts per crate — `INERT_HOST_HINT` is
that shape, and `is_framework_owned` is the same shape for the *level*: it lived
at one site of five, so two demo apps warned every boot about an indicator the
framework owns.

A `proc-macro` crate can only export macros, so each decorator lives in
a companion `*-macros` crate re-exported by its home crate. **That is
the one licensed exception to "`lib.rs` carries no logic"** — Rust
forces `#[proc_macro_attribute]` items to the crate root, so a
`*-macros` `lib.rs` holds them and they stay thin delegations into the
crate's own modules. The rule shipped to products has no such carve-out,
correctly: a generated project has no macro crate. Shared token
helpers in `nest-rs-codegen`. A `*-macros` crate **must not** depend on
its surface crate — emit absolute-path tokens; never rely on call-site
scope. Testable form: **a `*-macros` crate emits only `::std`/`::core`
paths or paths routed through its surface crate's re-exports
(`::nest_rs_<x>::<dep>`) — never a bare third-party path** (`::anyhow`,
`::tracing`, …), which resolves against the *consumer's* extern prelude
and breaks any app lacking that direct dep.

**The root is the umbrella, not the sibling.** A `*-macros` crate emits
`::std`/`::core` paths, or paths rooted at `::nest_rs::<concern>::` —
never `::nest_rs_<sibling>::` directly, and never a bare third-party
path. Non-API seams stay `#[doc(hidden)]` in the crate that owns them;
the root is what carries the contract. The developer declares
`nest-rs` with the capability's feature and nothing else; see *The
umbrella is the front door* in `CLAUDE.md`. Routing through the umbrella
is also what dissolves the cycles — `nest-rs-guards`, `nest-rs-authz`
and `nest-rs-seaorm` sit *above* the transports, so a sibling root can
never reach them, while `::nest_rs::` sits above all of them.

Two exceptions survive, and neither is a licence:

- **Emitted derives** (`::serde`/`::validator`/`::schemars`). A derive's
  own expansion targets the call-site prelude, so re-export routing
  would be false hygiene. The fix is the derive's `crate = ` override
  plus a re-export from the surface crate; until a given derive has it,
  the path is legal **only when the developer's own source writes that
  derive**. Same for the entity-site trio `::sea_orm`/`::uuid`/
  `::chrono`: an entity file names them itself.
- **poem's `#[handler]`**, which `#[routes]`/`#[crud]` wrap and whose
  expansion targets the call-site prelude. This one is a **known defect,
  not a design** — a controller crate should not have to declare `poem`.
  It is reported on `/http/`, never argued away.

"The use site owns that crate by definition" is not an admissible
reason. Owning a capability means enabling its feature.

The proof is compile-time: `nest-rs-macro-hygiene` (workspace,
`publish = false`) consumes decorators with **zero** third-party deps —
extend it when adding a decorator. It holds **decorators only**: a module
import there proves nothing about a macro and squats a proof that belongs
in the owning crate's own suite (see *Shipping a new capability* step 5
in `CLAUDE.md`).

It deliberately does **not** consume `#[crud]`/`#[expose]`: those need a
real entity, and an entity cannot live in a zero-dep crate —
`DeriveEntityModel` roots its expansion at the call site's `sea_orm` and
offers no `crate = ` override, which *is* the entity-site exception. Their
contract is proved by `crates/nest-rs-cli/tests/e2e/scaffold.rs` — which
scaffolds a workspace, generates a resource, repoints `nest-rs` at the
working tree through `[patch.crates-io]`, and runs a real `cargo check`.

**The two are excluded for different reasons, and conflating them cost a
shipped defect.** `#[expose]` sits on an entity, whose own source
legitimately writes `sea_orm`. `#[crud]` sits on a **controller**, whose
source writes nothing but `std`, `nest_rs` and `crate::` — it has no excuse,
and it emitted `::uuid::Uuid` for three routes and one resolver argument.
`g resource` bootstraps `g auth`, whose claims type names `uuid`, so the
scaffold e2e had the dependency whether the macro needed it or not, and
passed throughout. **A generated tree witnesses only what it does not also
supply by accident**:
`crud_needs_no_dependency_the_controller_does_not_name` drops the auth
modules from the module tree, the guards from the controller and `uuid` from
the manifest, so what remains rests on the decorator alone.

**The path-rooting rule is now executed, not merely stated.**
`nest-rs-macro-hygiene/tests/integration/emissions.rs` reads every
`*-macros` source and fails on a path rooted outside the framework — an
allowlist of the framework's roots, not a list of banned crates, so a
decorator reaching for something nobody thought to ban fails the day it is
written. The scan is exhaustive over decorators and blind to feature
resolution; the compile witness is the reverse. Keep both, or the next
defect hides in the gap between them.

**That distinction is load-bearing.** The `integration` suite asserts on
the *text* the generator wrote, which catches a wrong dependency and can
never catch a template that emits code the compiler rejects — only a user
would find that. A template change is not done until the `e2e` suite has
run; it shares the repo's target directory, so a warm run is seconds.

### One decorator, one item shape

**An edge is a pair of decorators, never one name worn twice.** The hard "no"
in `CLAUDE.md` carries the reasoning; here is the table it binds, and it is
closed:

| Edge | on the struct | on the impl |
|---|---|---|
| HTTP | `#[controller(path)]` | `#[routes]` (or `#[crud]`, which re-emits under it) |
| WS | `#[gateway(path)]` | `#[messages]` |
| GraphQL | `#[resolver]` | `#[operations]` (or `#[crud]`) |
| MCP | `#[mcp]` | `#[tools]` |
| queue / schedule / events | `#[injectable]` — no mount, no provider-scope layers | `#[processor]` / `#[scheduled]` / `#[listeners]` |

**The struct half is named for the host role; the impl half for what it
collects.** `#[messages]` carrying `#[on_connect]` beside the message arms is
the precedent for the dominant-unit reading, and it is why `#[tools]` is right
for a block that also holds `#[prompt]` methods: rmcp routes both through the
one `ServerHandler` the expansion writes, so they are one host's operations.

**MCP is the one edge whose *struct* decorator is not its role word.** The role
word went to the impl half, where a host's methods are; `#[mcp]` keeps the
protocol's name, and cannot be misread as the `#[tool]` this crate re-exports
and which the same file carries. The role word is in the file (`tool.rs`) and
the module (`<Feature>McpModule`) too. Accepted asymmetry, not an oversight —
and **not a licence to make either name cover both shapes again.**

`#[tools]` is what absorbs rmcp's three-block shape — `#[tool_router]`,
`#[prompt_router]`, `#[tool_handler]`/`#[prompt_handler]`, `get_info` — into
generated code, and it earns its keep three ways beyond the line count:

- **`use rmcp;` leaves the developer's file.** rmcp's macros resolve bare
  `rmcp::` paths against the call site, so a host had to carry an import whose
  only job was someone else's hygiene. The expansion emits those impls inside a
  private child module that carries the import itself. Two Rust facts make it
  sound and both are asserted in `nest-rs-mcp/tests/integration/mcp_impl.rs`:
  an inherent impl may live in any module of the defining crate (a descendant
  still reaches the parent's private fields), and an item's **own visibility**,
  not the module it sits in, decides who may name it.
- **That second fact is load-bearing, not trivia.** rmcp generates
  `tool_router()` *without* `pub`, so reading it from the parent silently yields
  an empty tool list — the duplicate-tool boot check would go blind. rmcp
  answers that itself (`#[tool_router(vis = "pub(crate)")]`), and
  `DefaultToolRouter` / `DefaultOperationLayers` are the empty fallbacks the
  struct half falls through to for a host that has no `#[tools]` block.
- **Capabilities are derived, never restated.** A `#[tool]` method advertises
  `tools`, a `#[prompt]` method `prompts`. A host can no longer route
  operations it forgot to declare — the defect the CLI template itself shipped.

**The escape hatch is a host that owns its `ServerHandler`.** Resources,
completion and the rest are hand-written trait methods, and the sugar cannot
generate a second `impl ServerHandler`; such a host writes rmcp directly and has
no `#[tools]` block at all — `#[tools]` on a trait impl is a compile error
saying so. `demo`'s `posts` is that host, deliberately kept as the witness of
the raw shape.

### The impl half — one method grammar at every member

**Every impl-half decorator reads a method the same way**, because a developer
moves between them inside one feature: `#[routes]`, `#[messages]`,
`#[operations]`, `#[tools]`, `#[processor]`, `#[scheduled]`, `#[listeners]`,
`#[indicators]`, `#[hooks]`. Five facts hold at all nine, each worded once in
`nest_rs_codegen`, with a trybuild snapshot per decorator:

- **One role per method** (`one_role_per_method`). A second role attribute — a
  second verb, a second trigger — is a compile error with the caret on the
  repeated attribute, the span chosen by the helper and not by the caller; never
  the first taken and the rest left on the method.
- **The receiver is a shared borrow** (`shared_receiver`). A host is one instance
  shared by every call, so `&mut self` and `self` cannot be served, and a method
  with no receiver is refused — never has its first argument skipped as if it were
  one. The provider-hosted five also take `self: &Arc<Self>`, because they hold
  the host's `Arc` and may lend it; the four edges take `&self` alone, because WS,
  MCP and GraphQL call a method on `&Self` and have no `Arc` to lend — a recorded
  asymmetry, and the sentence names which borrow each member takes.
- **`fn` and `async fn` are both accepted**, and only an `async fn` is awaited.
  Whether a body blocks is the developer's call, as it is in any Rust function;
  refusing a synchronous method would assert an impossibility no standard holds,
  and `#[operations]` and `#[tools]` already served both.
- **A dispatched method is concrete** (`concrete_signature`). It takes no type
  or const parameter, because the expansion calls it with only what its caller
  carries and nothing supplies one; lifetimes are allowed.
- **`#[cfg]` travels with the method** (`cfg_attrs`), including a `cfg` inside a
  `cfg_attr`, and nothing else does: a method compiled out is compiled out of its
  wrapper, its registration and its inventory entry.

**A key one member of a decorator family takes is answered at every member.** The
worker-job family — `#[process]`, `#[every]`, `#[cron]`, `#[after]` — is the
worked case, and its table is closed: every family key is built at a member, or
refused there with the fact that makes it meaningless (``#[every] takes no
`retries`: a tick's retry is the next occurrence``); a key no member takes keeps
the unknown-key sentence, listing that member's column. **The table is declared
once, in `nest_rs_codegen::job`** — the members (`JobDecorator`), the keys
(`JobKey`), and `cell(key, member)` placing every key at every member — and it is
a `match` with no wildcard arm, so a key or a member added without a cell at every
crossing does not compile, and a cell written twice is an unreachable pattern. A
new key therefore never reaches the other members as an unknown word. Every member
reads its keys through `job_key`, which answers taken, refused or unknown from
that one table; each parser matches on `JobKey`, so a key cannot reach a parser
without joining the table; and each member's own test reads every key of its
column in the spelling the table offers (`JobKey::example`), so a key the table
gives a member and its parser does not read fails a test — `unread_job_key` is
that sentence, never a panic. `transactional` is the one key every member builds.
Every refused cell is pinned by its decorator's `*_refused_keys` trybuild
snapshot, and every built one compiled by a use site in `nest-rs-macro-hygiene`.

Two of the schedule's refusals are decisions rather than impossibilities, and they
are this framework's recorded contract, not a gap: a tick has no retry — its retry
is the next occurrence — and never overlaps itself — an occurrence falling inside a
run is skipped and counted. A tick whose work must not be lost pushes a queue job,
which is delivered at least once and retried there.

### When (not) to write a decorator

**Write one when all three hold:** the pattern appears in ≥ 3 places;
the boilerplate is mechanical; the rule is teachable in one sentence.

**Never for:** business logic; one-off integrations; context-dependent
inference Rust can't give (prefer a builder); anything needing `unsafe`
or runtime reflection.

Ships with: a doc comment showing the expansion; a test in the home
crate's `tests/` (or `nest-rs-testing` for cross-crate wiring); a use
site in an app or `features`. **Compile cost > 0.5 s per use site is a
defect. Measure.**

## The DI container is internal

Surveyed the ecosystem; none met our bar. **Do not propose an external
DI crate.** Extend ours.

### Composition model

- **`App::builder().build().await` runs four phases** independent of
  call order: *seeds* (runtime values from `main`), *collect* (modules
  queue async factories), *factories* (awaited; seed wins over factory
  of same type), *register* (providers built, injecting seeds + factory
  outputs). `main` holds only `App::builder().module::<AppModule>()`
  (+ transports). Sync apps keep `App::new`.
- **Providers are singletons** unless scoped. Two non-default scopes:
  - `#[injectable(scope = request)]` — built per request, deps from the
    singleton root. **One level deep**: request-scoped may inject
    singletons; never the reverse or another request-scoped. Reach one
    through the request boundary (`nest_rs_http::Scoped<T>`,
    `nest_rs_graphql::Scoped<T>`, `nest_rs_mcp::Scoped<T>`), never via
    `#[inject]`.
  - `#[injectable(scope = transient)]` — rebuilt on **every** resolution,
    no caching. May depend on singletons or request-scoped. A transient
    that transitively depends on itself **panics at resolution** with a
    cycle diagnostic naming the chain — the one provider error caught at
    first-resolution rather than at boot. Singleton is the default;
    reach for transient only when a fresh instance per use is genuinely
    required.
- **Modules compose by type or configured value.** `#[module(imports =
  [...])]` takes a bare type or a call like `OpenApiModule::for_root(opts)`
  (`DynamicModule`). Configure via `register` (sync) or `collect` (async
  factory). Registration is **idempotent** (diamond imports build once);
  dynamic imports are **not** deduplicated.

### `for_root` — one seam, one value, no chain

**A module is configured in exactly one place, by exactly one value.**
`Module::for_root(x)`, and `x` carries *everything* the app declares
about that module. The `DynamicModule` it returns is **opaque**: no
public method on a `*Setup`, no second constructor on the module type.
A declaration that does not fit into `x` makes `x` grow a **field** —
never the seam a **method**. A builder chain (`for_root(None).thing(t)`)
is three spellings of one import, and the second constructor added to
soften it (`Module::thing(t)`) is the fourth; both are defects.

Testable form, both halves checkable: **`rg 'impl \w+Setup' crates/`
returns nothing**, and **no module type has an inherent `pub fn` besides
`for_root`** — with `ConfigModule` the single carve-out, because it is
the config crate itself rather than a configurable module. Its
`for_root` / `for_feature` / `provide_feature` / `setup` are the
primitives every other module's seam is *built from*, and
`provide_feature` is public API a third-party driver calls
(`docs/…/database/writing-a-driver.mdx`). Nothing else gets that
exemption: `nest_rs_throttler::provide_guard` / `resolve` are the same
kind of cross-crate seam and they are `#[doc(hidden)]`.

Two shapes for `x`, and only two:

- **`impl Into<Option<C>>`**, `C` being the module's `#[config]` — the
  default, and what almost every module wants. `None` ⇒ env over
  `C::defaults()`; `Some(c)` ⇒ env over `c`, per field.
- **`impl Into<MOptions>`**, where `MOptions { config: Option<C>, /* … */ }`
  is a plain `Default` struct declared beside the setup in `module.rs` —
  only when the module carries a declaration that genuinely has **no env
  twin** (`McpIdentity`). It keeps `From<C>` and `From<Option<C>>` so the
  config-only call site reads exactly like every other module's.

**Don't hand-write the setup when the shape is plain.** A `for_root` whose
whole job is "pin the config, then recurse into my own wiring" returns
`ConfigSetup<M, C>` — `pub type WsSetup = ConfigSetup<WsModule, WsConfig>;`
plus a one-line `for_root` calling `ConfigModule::setup(config)`. Keep the
alias: the name is what the docs and `for_root`'s signature reference. Write
your own type only when `collect` queues more than the config (a pool, a
client) or `register` does more than recurse (`McpSetup`, `HttpSetup`,
`GraphqlSetup`, `OpenApiSetup`). The constructor lives on `ConfigModule`
rather than as `ConfigSetup::new`, so a *shared* setup is as opaque as a
hand-written one and the first half of the testable form still holds.

The `Option` inside `MOptions` is load-bearing, not a habit: `Config::resolve`
ranks the `.env` cascade *below* a pinned base and *above* `defaults()`, so
flattening it to a bare `C` would silently demote the cascade.

**This does not outlaw the bare import.** `imports = [WsModule]` is a
*dependency declaration* — "my providers inject `Arc<WsServer>`" — and it
configures nothing, so it is not a second seam. A module that must not be
imported bare hides its `#[module]` behind a private host struct and
exposes only the plain façade: `OAuthResourceHost` /
`OAuthResourceModule` is the exemplar.

**The ownership table — which config reaches which seam — is in
`architecture.md`** (*Configuration — one seam per config*), because a product
developer needs it too and that file is loaded in every session. Read it there.
Its one line for this crate: **every `nest-rs-*` module owning a `#[config]`
owns a `for_root`**, because a consumer cannot edit your `Default` and that seam
is their only in-code path. The obligation stops at the crate boundary — a
product's own module already has `impl Default`. Three points belong here, with
the reasoning the table omits:

**`for_root` configures; `for_feature` registers.** This is NestJS's split, and
it is not stylistic: `forRoot` configures a module once, `forFeature` registers
artifacts against an already-configured one (`TypeOrmModule.forFeature([User])`
mounts repositories, it does not reopen the connection). Our `for_feature` takes
no value for that reason. It briefly took a pinned base, and that was the defect
— it made every module-owned config reachable two ways, which is the *second
seam* this whole section forbids.

**A module that owns no `#[config]` gets no `for_root`** — the sharper half.
`SocialModule` is the case that proves it: a provider carries its own
`#[config]` and the registry entry names it, so discovering the provider is what
loads its credentials. The module never learns which providers exist, so there
is nothing for it to be configured about. Giving it a `for_root` anyway forces a
list of mutually unrelated config types, hence type erasure, hence no duplicate
detection and a hand-written `Debug` to keep a client secret out of the format —
all paying for a declaration the discovery seam already made unnecessary.

**The rule is enforced, not merely written.** A value an import site *chose* is
queued as a **declaration** (`ContainerBuilder::provide_declared_factory`,
which takes the remedy sentence its error will print). Three consequences, each
with a witness in `nest-rs-config`:

- a declaration **supersedes** an ordinary factory for the same type, wherever
  the two fall in `imports = [..]` — `a_pin_survives_a_bare_import_listed_before_it`;
- two declarations for one type raise `ContestedDeclarationError` before any
  factory runs — `two_pinned_bases_for_one_config_fail_the_boot`;
- the synchronous `App::new` refuses a queued factory it could never drain
  (`UnresolvedFactoryError`) — `the_synchronous_boot_refuses_a_config_it_could_never_resolve`.

**It is not config-only.** Any *adapter* binding an implementation a sibling
adapter also binds declares it, so importing both is a named boot failure
rather than whichever `imports` listed first — every vendor binding of
`Arc<dyn ThrottlerStore>` shares one `BACKEND_REMEDY` constant so the halves
cannot drift. **The port's own default is the one exception, by design**: it is
an *ordinary* factory (`ThrottlerModule` binds `InMemoryThrottler` that way), so
a vendor binding supersedes it wherever it sits in `imports` — the app writes
`ThrottlerModule::for_root(None)` for the policy and adds `RedisThrottlerModule`
to move the counters off-process, and no line has to be removed.

This is where we deliberately exceed NestJS, which lets the last registration
win in silence — a dropped declaration is on the wrong side of *no silent
failure*.

**`nest-rs-config` loads; the consuming module judges.** The config crate
resolves each variable through the tiers one variable at a time — deployment,
then the `.env` cascade, then defaults — except that a config pinned in code
switches its whole namespace to deployment over pin: beside a pin the `.env`
cascade is not read at all. It hands the values over, assigns them no meaning
and never arbitrates between variables or tiers.

**`<KEY>_FILE` is a spelling of `<KEY>`, never a second variable, and every
namespaced variable (`<PREFIX>_<NS>__<KEY>`) has it.** The kernel's own family
read before any container exists — `<PREFIX>_LOG*`, `<PREFIX>_ENV` — is not read
through the loader and does not. A value may be given inline or as the path of a regular file
holding it — the container-secrets convention (Docker secrets, Kubernetes secret
volumes) — so no secret ever has to sit in the process environment, where
`/proc/<pid>/environ`, a crash dump or a child process reads it. The file is read
through the loader's one bounded reader (a regular file, never a FIFO, at most a
mebibyte), and a consumer that re-reads on renewal reuses that reader and the
path the value came from. **The deployment chooses the spelling**: when
either spelling is present in the deployment tier — empty included — both are
read from the deployment alone, so a `.env` value under the other spelling is
shadowed exactly as one under the same name is. Both spellings set within the
tier that answers is refused naming both, because it is one variable given
twice, and the loader is the only site that sees two spellings — a consumer is
handed one value. A file holding nothing but line breaks is unset, and a refusal
names the spelling that was set and never repeats a value read from a file. That is the loader's one refusal, and
it judges spelling, never meaning; `ContestedDeclarationError` and
`ContestedVariable` refuse two declarations and two readers, never two values.

**A combination of different variables is the consumer's, validated once.** Half
a key pair, a secret beside asymmetric keys, a partial credential set: refused at
boot by the module that uses them, naming what is set, never settled by choosing
one — and checked **in the one constructor every path reaches**, a config-driven
boot and a value built in code alike, so the check has one site and its sentence
names the settings the caller actually wrote. Ranking the tiers of related
variables inside the loader was tried and removed.

**A duration a deployment sets has a floor and a ceiling**, and a value outside
them fails the boot naming the variable — pinned in code or read from the
environment alike, since it breaks the module the same way from either side, and
never clamped in silence. The floor is where the thing stops working: a lease too
short to renew, a poll that spends Redis hundreds of scripts a second, a zero
budget that gives up before its first attempt. The ceiling is where a typo stops
being a setting: an orphan threshold in the millions delays crash recovery by
weeks, and past a few hundred thousand years apalis panics in its heartbeat. **No
value the boot accepts may panic a library below it**, and the ceiling is what
makes that true. **One reader, one sentence:** every bounded duration is read
through `nest_rs_config::DurationBounds` — key, the field that pins it, unit,
floor and ceiling each with its reason — so the environment and the pin are
refused by the same code in the same words; a constructor a hand-built value
reaches without a config read (`RedisConnection::connect`, `JwtService::new`,
SeaORM's connect) holds it to the same range through `DurationBounds::check`.
Three shapes were written by hand before it, and the one that skipped the pin
booted a zero Redis budget into "could not reach Redis … within 0ns". Built:
`RedisWorkerConfig`'s `orphan_after` (5 s to a day), `lease` (1 s to half the
orphan threshold), `poll_interval` (10 ms to the orphan threshold) and
`shutdown_timeout` (1 s to an hour); `HttpConfig::shutdown_timeout` (1 s to an
hour); `AUTHN__EXPIRES_IN_SECS` (1 s to thirty days) and `AUTHN__LEEWAY_SECS` (0
to 300 s, RFC 7519 §4.1.4's "a few minutes"); and, floor only,
`SEAORM__CONNECT_TIMEOUT_SECS` and `THROTTLER__WINDOW_SECS` (1 s),
`HEALTH__INDICATOR_TIMEOUT_MS` / `PROBE_DEADLINE_MS` (1 ms) and
`REDIS__CONNECT_TIMEOUT_SECS` — a whole second from the environment and anything
above zero in code (`Floor::AboveZero`), since that budget also bounds every
command and a sub-second one set in code is a fail-fast choice. A floor is
`Floor::Units` when it is a property of the thing bounded — a lease needs a
second to renew in — and `AboveZero` when zero is the only value that fails.
**The ceilings still missing are owner questions**, each a one-line change once
decided: those five floors' ceilings; HTTP's
`REQUEST_TIMEOUT_SECS`, `SSE_MAX_CONNECTION_SECS` and `SSE_KEEP_ALIVE_SECS`,
`WS__MAX_CONNECTION_SECS`, `GRAPHQL__MAX_CONNECTION_SECS` and MCP's
`SSE_KEEP_ALIVE_SECS` / `SSE_RETRY_SECS` (all read through
`ConfigService::seconds`, where `0` means off, so their floor is a second and they
stay outside `DurationBounds`, which has no off switch: every setting it reads
has a floor because *off* is the defect it bounds); `HTTP__TLS_RELOAD_SECS`; and
`OPENTELEMETRY__METRIC_INTERVAL_SECS` (`0` keeps the SDK's default). A throttle
written in code — `Throttle::new` — refuses a window under a millisecond, the
Redis store's resolution: a zero window reset every bucket on every hit and let
every request through at any limit.

**A variable no config claims is reported, never ignored.** A deployment that
misspells a variable, or keeps a name a release renamed, gets the default — and
without a report, no signal, since nothing ever asks for the value. So the loader
reports two shapes at `warn` on `nest_rs::config`, once per variable, **by name and
never by value**: a key under a namespace this binary read that no config read
(`UNREAD_CONFIG_VARIABLE`, with the nearest key that was read as `suggestion`
when one is near enough), and a near miss of a linked namespace
(`MISSPELLED_CONFIG_NAMESPACE`) — other separators (`OAUTH_RESOURCE` for 7.0's
`oauth__resource`, `SEAORM_URL` for `SEAORM__URL`), another case, the prefix's
included, since the loader folds none, or one misspelled segment, a family
member's included (`PROBE__MEMBR` for `probe__member`). **Everything else is
silent, by design**: one `.env` serves several binaries, so a namespace this
binary does not link and that is no near miss of one it does is another binary's,
never a mistake. The near-miss reach is narrower than a key's for that reason —
one segment, a quarter of it — and the `no_namespace_is_a_near_miss_of_another`
join holds every namespace of both workspaces outside it of every other, with
the report's own function. **A namespace belongs to one type**: two `#[config]`
structs declaring it are refused at the read of either, naming both
(`ConfigError::SharedNamespace`) — the key check ran once the first had been
read and filed the second one's keys as read by nothing, on a correct
deployment. The key half runs inside `read`, the one funnel
into every `from_env`, once a `from_env` has returned — a config's keys are
knowable only where its hand-written reader runs (`HttpCors` reads five of its six
keys only when `CORS_ORIGINS` is set), so a process-wide dry run of every config
would report a correct deployment, and a diagnostic that fires on correct
configuration teaches operators to filter its target out. **`nestrs doctor` does
not run it**: doctor links no framework crate, so it cannot know which namespaces
and keys the app's binary links, and any list it carried would be a second
authority on the framework's variables.

**One recorded exception: `OpenTelemetry::init_with(config)`.** The global
tracer and meter must exist *before* any module registers (the module panics
otherwise) and the returned guard's `Drop` flushes, so it belongs to `main`
and cannot be an import. Any other module claiming an exception is reported,
not written.

### Access contract (compile-time + boot-time)

- **Visibility is Rust's job.** Flat container ⇒ hide impls
  module-private, expose a `pub trait` bound with `provide_dyn`.
  Consumers inject `Arc<dyn Trait>`. **No `exports` list.**
- **Import contract enforced at boot** by the access graph
  (`crates/nest-rs-core/src/access.rs`): `#[module]` records imports and
  each provider's injected `TypeId`s into `inventory`; `App` walks from
  the root and fails boot (`AccessGraphError`) if a provider injects
  something its module doesn't own, import transitively, or receive as
  global infra (seeds + factory outputs). Governs `#[inject]` **and**
  `#[use_guards]`/`#[use_filters]`/`#[use_interceptors]`. Runtime
  `Container::get`/`get_dyn` is an unchecked escape hatch — the contract
  binds the declarative surface only.
- **Single flat container** — no per-module sub-container. Orphan rules
  prevent accidental coupling.

### Discovery

Module-wired items implement `Discoverable`; modules list them flat in
`#[module(providers = [...])]`. Single-concern decorators
(`#[injectable]`, `#[mcp]`, gateway struct) emit `impl Discoverable`
directly. **Inventory-based** — the module list *is* the decorated
things; never enumerate controllers/providers by hand.

**Orchestrator pattern for per-method aggregation:** `#[routes]` scans
verbs, `#[operations]` scans `#[query]`/`#[mutation]`/`#[subscription]`/`#[entity]`/`#[field_resolver]`,
`#[scheduled]` scans `#[every]`/`#[cron]`/`#[after]`, `#[processor]`
scans `#[process(queue, ...)]`, `#[listeners]` scans `#[on_event]`,
`#[hooks]` scans phase attrs. The host struct owns the single
`Discoverable`; each method submits its unit to link-time `inventory`.
Use this for any concern where one provider owns several units sharing
the same `#[inject]` deps. Otherwise stay struct-level.

**Discovery is module-gated.** Every transport integrates only items
whose provider is *reachable* from the running app's root — a
`ReachableProviders` set from the access graph; each transport filters
its `inventory` against it. Linked but unreachable ⇒ inert, with a boot
`tracing::warn` so leftover code doesn't vanish silently. This is what
makes per-app subsets work.

**In a workspace of several apps that `warn` misreads one case, and which way
to settle it is an owner question.** Two binaries linking one feature library
each import the hosts they serve, so each warns about every host its sibling
imports — the demo's api about the worker's `NotificationsTasks`, the worker
about the api's `AudioTasks`, the assistant about two listeners — with
`INERT_HOST_HINT`'s "import it, or delete the methods", which is wrong advice
when another binary hosts them. The config report settled the same shape the
other way: a namespace this binary does not link is another binary's, never a
mistake. Possible and unbuilt: a host from a library crate the binary links but
never imports the module of reported at `debug`, with `warn` kept for the
binary's own crate — or, on the product's side, every host of one edge kept in
one app.

**The gate is always the entry's owner** — what differs is who the owner
*is*, and that follows from what the entry is:

- **An entry that names a DI provider** — a method or role on something
  `#[inject]`ed by type (`#[process]`, `#[on_event]`, `#[query]`,
  `#[every]`, `HealthIndicator`) — is owned by **that provider**, so the
  gate is `ReachableProviders`.
- **An entry that names no provider** — a self-contained plugin the
  registry builds from its own config, like `SocialProviderEntry` — is
  owned by **the module providing the registry** (`SocialModule`), whose
  presence in the import graph is the gate.

Same rule either way, and it is why the second kind needs **no module of
its own**: only something injected by type does.

**Being buildable is not discovery.** Once an entry is discovered, its
own config decides its fate on the dual-path `#[config]` rule: complete ⇒
active, absent ⇒ inert + boot `warn`, partial/invalid ⇒ boot fails naming
it. Never conflate the two — a capability that cannot be constructed is
not "undiscovered".

**Structural gating where discovery is metadata.** `ReachableProviders`
exists because `inventory` is *link*-time: everything compiled is in the
registry, imported or not. Metadata attached from `Discoverable::register`
has no such gap — `register` only ever runs for a provider an imported
module owns — so a metadata-discovered surface (`HttpEndpointMeta`,
`McpHostMeta`) is gated by construction, with nothing to filter and no
inert-entry `warn` to emit. Pick the mechanism, then take its gate; never
bolt a `ReachableProviders` filter onto metadata to look symmetric.

### A key a datastore holds is the span target, written for that store

Anything the framework writes into a shared datastore — a Redis key today — is a
**name an operator types**: in a chart's KEDA trigger, in a `SCAN` during an
incident, in an ACL. So it obeys the naming law like any other name, and it is
derived rather than chosen:

```
nestrs:<concern>:<structure>[:<member>]
```

- **`<concern>` is the tail of the span target of the crate that owns the
  concern** — `nest_rs::queue` → `queue`, `nest_rs::schedule` → `schedule`,
  `nest_rs::throttler` → `throttler`. *Owning*, not writing: `nest-rs-redis`
  writes every one of these keys and `redis` names none of them, because an
  operator looking at Redis is looking for the queue's keys, not "the Redis
  crate's".
- **`<structure>` is what the key is within the concern** — `buckets`, `claims`,
  `leases`, `settled` — one word, never the concern's own word said again, and
  inside a queue's namespace never a word apalis uses for a structure of its own.
- **`<member>` is what varies** — a queue, a job's id, an occurrence, a client's
  bucket. A queue name holds no `:`, so a queue is one level.

**It is the fourth column of a table that had three.** The crate's subject, its
span target and its `#[config]` namespace are one derivation, and a datastore key
is the same derivation reaching one more surface — so from a key a reader names
the port crate that owns it, and from that crate derives the key:

| surface | derivation | example |
|---|---|---|
| crate | the subject | `nest-rs-throttler` |
| span target | `nest_rs::<concern>` | `nest_rs::throttler` |
| env namespace | `<PREFIX>_<CONCERN>__*` | `<PREFIX>_THROTTLER__*` |
| datastore key | `nestrs:<concern>:<structure>` | `nestrs:throttler:buckets` |

Three obligations, each for a mechanical reason. The `keys` join in
`nest-rs-conformance` executes as much of them as a literal can show — every
literal opening with `nestrs:`, macro bodies included, over both workspaces and
the docs — and its baselines are empty and only shrink:

- **Every fixed part is a `const` whose literal opens with `nestrs:`**, declared
  by the crate that writes the key, and a key that varies is built from exactly
  one such constant — a template whose slots are filled (`nestrs:queue:{queue}`)
  or `format!("{CONST}:{member}")`. A fixed segment written after a `{CONST}` in
  a format string is a key nothing checks.
- **No key prefixes another without a visible level.** `SCAN` and `KEYS` match by
  glob, so `<ns>:queue` beside `<ns>:queue_configs` means the pattern an operator
  types — `<ns>:queue*` — returns both: the hazard `EnvFilter`'s `starts_with`
  makes of span targets, one surface over. A qualifier goes one level down, never
  alongside.
- **A key spelled outside Rust is built from one the code declares.** A KEDA
  trigger, a `NOTES.txt` `LLEN` or a docs page's `SCAN` never moves when a
  constant does, and a trigger polling a list nobody fills reads zero forever,
  without a word. A declared name's `{}` level stands for any one member
  (`nestrs:queue:audio:active` is built on `nestrs:queue:{}`), and an apalis
  structure at the root of the keyspace (`audio:active`) is refused — in Rust
  unless a placeholder opens it, which is how the 6.x check builds its names from
  the queue, and outside Rust even then. So a page describing the 6.x layout
  builds those names from a shell variable rather than writing one.

**The leading segment is `nestrs`, hard-coded, and that is settled.**
`NESTRS_ENV_PREFIX` renames every environment variable because those are the
developer's surface, sitting in their charts and secret stores. A key is the
framework's own machinery: two deployments sharing one Redis are separated by the
logical database in the connection URL (`redis://host:6379/2`), which
`RedisConfig` already parses — that is the isolation, and a prefix knob would buy
symmetry and nothing else.

**apalis's structures are apalis's.** A queue hands apalis the namespace
`nestrs:queue:<queue>`, and apalis derives its own structures from it —
`…:active`, `…:inflight:<worker>`, `…:scheduled`, `…:data`, `…:dead`, `…:done`,
`…:failed`, `…:signal`, `…:consumers`. The framework reads and writes those only
through apalis's public API, never with a command or a script of its own — and
never names one by hand: a name it needs is read off `apalis_redis::Config`'s
getters, so it moves when apalis's derivation does (the keys join's
`no_framework_code_spells_an_apalis_structure_by_hand` fails on a literal that
spells one under `crates/*/src/`) — and files its own records beside them, one
structure per fact, under words apalis does not use. An apalis behaviour the framework cannot live with is worked around
in keys of its own and reported upstream.

| concern | key | holds |
|---|---|---|
| queue | `nestrs:queue:<queue>` | the namespace apalis derives its structures from |
| | `…:open:<job_id>` | a job pushed and not yet settled, with its unique key — how a cancel tells a waiting job from one long finished, since apalis keeps a finished job's record until something vacuums it |
| | `…:leases:<job_id>` | the delivery running the job |
| | `…:settled:<job_id>` | how the job ended, so a later delivery is answered as the first was |
| | `…:cancelled:<job_id>` | a cancel's promise that the job never starts |
| | `…:checkpoints:<job_id>` | the progress the job saved |
| | `…:attempts:<job_id>` | the attempts started at the job, less those a drain handed back unrun — how the port counts an attempt whose process died, which the envelope never could |
| | `…:unique:<key>` | the job holding a unique key |
| | `…:throttle` | the attempts started in the current window, less those handed back unread (a `Defer`, taken back only inside the window that counted it) — one per queue, since one method drains a queue |
| throttler | `nestrs:throttler:buckets:<subject>` | one client's current window |
| schedule | `nestrs:schedule:claims:<occurrence>` | an occurrence's claim — the port's token (`<module path>:<provider>:<method>:<instant_ms>`, the declaring module's path a level per `::`) verbatim; the key's existence is the claim, and its value names the claimer and its run for operators only |
| | `nestrs:schedule:leases:<job>` | the run of the job going on now — the port's job identity (`<module path>:<provider>:<method>`) verbatim, held by the run that claimed an occurrence, renewed while it lasts and released when it ends |

**Nothing is kept forever, and nothing that is still owed lapses early.** Every
record of a job still waiting lives a week past the instant the job is due
(`KEPT_PAST_DUE`), renewed by every delivery that touches it — the bound on a
unique key whose job vanished. **A unique claim is the one record taken shorter**:
it is held for `CLAIM_HOLD`, twice the port's `BACKEND_TIMEOUT`, and extended to
the week once Redis confirms the filing, because a push that never learns
whether its job was queued — a claim or a filing unanswered, a call dropped by
its caller or by the net — must not refuse every retry under the key for a week
in the name of a job that was never filed; it says so at `warn`, naming the step,
and errs toward at-least-once. The week is a constant: a knob would need the
queue binding's first `#[config]` and its `for_root`, an owner question rather than
a default. A settled mark lives past the latest a sweep could hand the job to a
second delivery, `max(1 h, 2 × orphan_after + lease)` — and a week when its
acknowledgement may be lost, since a job whose acknowledgement apalis dropped
stays in flight until *some* replica starts, however much later: a job settled
during a drain, and every job settled within the span an acknowledgement takes
(`acknowledged_within`, a poll and a heartbeat) before the drain began or before
apalis reported one lost. A quiet longer than that week, or an acknowledgement
loop further behind than the span, is the residual, and stated as such. At about
120 bytes a job that is a documented cost, not a setting.

**The 6.x layout is refused, never read.** 6.x handed apalis the queue's bare
name, so its jobs sit at the root of the keyspace, where a 7.0 worker never looks,
and jobs left there would wait forever without a word. So the worker **refuses to
start** while one holds a job, naming the keys and both ways out — drain with 6.x
workers, or move with `RENAMENX` (never `RENAME`, which would overwrite what 7.0
already filed) — and the producer **warns** once per queue instead, because
refusing there would block the drain-first rollout, which upgrades producers
first. The check is apalis's own `stats`, on a storage opened under the queue's
bare name — the rule above holds for 6.x's structures too, so the names are
apalis's getters' and the read is apalis's script, never a command of the
framework's — and it counts the waiting list and the registered in-flight sets.
A key of another type at one of those names is refused by Redis (`WRONGTYPE`)
and read as not 6.x's, said at `info`, never counted as jobs nor printed as a
`RENAMENX` into apalis's list. **The 6.x schedule is not counted**: no public
apalis call counts a schedule, so the upgrading page has the operator count it,
and whether a read of it is worth an exception to the rule above is an owner
question. Never a `SCAN`, which costs the whole keyspace and which an ACL
confined to `nestrs:*` refuses; a `NOPERM` answer is said, never taken for an
empty layout. The move the docs print
is the one `layout::a_queue_moved_out_of_the_6x_layout_runs_every_job_it_held_once`
runs.

### A swappable concern ships an extension contract

Anything a third party could plug a different implementation into owes a
written **extension contract**: the trait(s) they implement, the seam that makes
their implementation reachable from the container, and the sentence that says
what happens when there is more than one. **If the contract cannot be written,
the concern is not swappable — say so in the crate's `//!`, never leave it to be
inferred.** A trait with no contract beside it is an extension a reader has to
reverse-engineer; a client with no trait and no sentence is a closed door that
looks like an open one.

**The unit is the port, never the crate that houses it.** A crate may hold a
port and something that is not one — `nest-rs-authn` holds `Strategy` (a port,
selected by type parameter, owing no namespace) beside `JwtService` (shared token
infrastructure four sibling crates reach, whose `NESTRS_AUTHN__*` is legitimate).
Ask the questions below of the port; asking them of the crate is what produced
the two false rows this table used to carry.

Two questions determine the shape, and they are **independent** — answer both.

**Q1 — who owns the driver problem?**

- **Delegated.** A third-party library is *already* the multi-driver
  abstraction, and the nestrs crate is a thin adapter over it that does not let
  the vendor's types leak. `object_store` (S3, GCS, Azure, local filesystem,
  in-memory) under `nest-rs-storage`; `sea-orm` (postgres/mysql/sqlite) under
  `nest-rs-seaorm`. The port then
  exists for exactly one move — swapping the **vendor** — so its contract is
  thin **and declares no config**, because nothing is asked of the integrator's
  settings. A thin port here is the correct outcome, not an unfinished one:
  reading `nest-rs-database`'s bare `Executor` as a deficiency is the mistake
  this paragraph exists to stop, and it was made three times in one session.
- **Owned.** Nothing abstracts the concern, so nestrs defines the trait, the
  registration seam and the arbitration sentence itself — `ThrottlerStore`,
  `OccurrenceLock`, `JobProducer`, `SocialProvider`, `Strategy`. `JobProducer` is
  the one that moved, in 7.0: apalis serves several stores, but what a job *is* —
  its id, its envelope, its attempt and retry budget, the capabilities a backend
  declares — is `nest-rs-queue`'s, and apalis is the Redis job runtime one binding
  drives. A second backend implements the port; it never configures apalis.

**Q2 — what selects the active implementation? Three modes, and the third is
the one this framework uses most.**

- **By import.** Exactly one, chosen at compile time by which module the app
  imports; the consumer injects `dyn Port` and never names the backend. Two
  imported is a **boot error** naming the port and the remedy, worded once and
  shared by every backend so the halves cannot drift —
  `nest_rs_throttler::BACKEND_REMEDY` is that shape already built. It does not
  name the two bindings; whether it should is the owner question *Swapping or
  adding a backend* records in `architecture.md`. A port carrying a `BACKEND_REMEDY` declares it **in
  the file holding what a backend supplies** — `nest_rs_queue::backend`,
  `nest_rs_schedule::occurrence`, `nest_rs_throttler::store` — never in a
  `module.rs`, because the contract is what a backend author opens and the module
  only passes the sentence on. The sentence may name the first-party binding as
  the remedy: it is advice a reader acts on, and *a port's dependencies name no
  vendor crate* binds the manifest, not the prose.
- **By type parameter.** The app names the implementation in an alias and the
  generic host is instantiated with it — `AuthnGuard<S: Strategy>`,
  `AbilityGuard<F: AbilityFactory>`, `GraphqlAbilityBridge<A, G>`. **No
  arbitration exists and none is owed**: two instantiations are two distinct
  types, so nothing can be contested. The port declares no config either; an
  implementation that needs one carries its own.
- **By configuration.** The import only **opens the gate** (module-gated
  discovery); **configuration decides which members are active**, from zero to
  all of them. Discovery registering an entry is not activation — that
  distinction is the whole mechanism, and `nest-rs-social` is the exemplar:
  a complete `NESTRS_SOCIAL__<KEY>__*` set makes a provider active, an absent
  one leaves it inert with a boot `warn`, a partial one fails the boot naming it.

The two axes are orthogonal, and the measured tree carries this much:

**Q1 is answered by the crate that *binds* the port, never by the crate that
declares it.** A port crate depending on nothing is the normal case —
`nest-rs-database`'s whole manifest is `tokio`, `nest-rs-queue` names no
`apalis` — so it is the driver row that says delegated or owned:

| port | declared by | bound by | Q1 (of the binding) | Q2 | contract written |
|---|---|---|---|---|---|
| `Executor` | `nest-rs-database` | `nest-rs-seaorm` | delegated (`sea-orm`) | by import | **yes** — `## Extension contract`, but no arbitration sentence |
| `JobProducer` / `CheckpointStore` | `nest-rs-queue` | `nest-rs-redis` | owned — apalis is the Redis job runtime the binding drives, not the port | by import | **yes** — the crate's `# Extension contract`, `docs/queue/writing-a-driver.mdx`, and `BACKEND_REMEDY` |
| `OccurrenceLock` | `nest-rs-schedule` | `nest-rs-redis` | owned | by import | **yes** — the port's `//!` in `occurrence.rs`, `BACKEND_REMEDY` its arbitration sentence |
| `SocialProvider` | `nest-rs-social` | itself + third parties | owned | by configuration | **yes** — open provider contract |
| `ThrottlerStore` | `nest-rs-throttler` | itself + `nest-rs-redis` | owned | by import | **yes** — the trait's doc (what `hit` owes within `HIT_TIMEOUT`), `BACKEND_REMEDY` beside it, and the declared binding under *Writing your own* on `/rate-limiting/` |
| `Strategy` | `nest-rs-authn` | the app's alias | owned | by type parameter | **yes** — *Advanced: write your own strategy* on `/security/authentication/`: the trait, and the alias that selects it; no arbitration is owed |
| object storage | — | `nest-rs-storage` | delegated (`object_store`) | **nothing selects** — see below | no port exists |

**A port promises only what every backend it ships holds.** A guarantee one
backend keeps more loosely is either refused by that backend — its
`QueueBackend` does not declare the capability — or worded in the port loosely
enough that every backend keeps it, and the backend's own bound is written on the
backend: `nest-rs-redis`'s `backend.rs` states its throttle window, the port says
only *starts per window, across the deployment*. A port sentence describing one
backend's behaviour has taken that backend's semantics for its contract.

**A port call the framework awaits is bounded by the framework.** A backend is
trusted to answer, never to answer in time: past its bound a call is treated as a
backend that cannot answer, and never waited on in silence, because a hung
backend is a job that stops or a request that stalls with nothing to say why.

**The bound is a net, never the backend's budget.** A backend still bounds each
command it sends — `RedisConnection` answers or fails every command within its
connect budget (`NESTRS_REDIS__CONNECT_TIMEOUT_SECS`, 10 s by default) — so an
outage arrives as the backend's own error, with its cause, and a net fires only
on a backend that stopped bounding itself. **A command whose answer is the only
record of what it claimed is never cut**, because a timeout does not undo it: the
Redis worker's fetch (which claims jobs into its replica's flight) and the
guard's admission (a lease, a throttle start, an attempt) wait for their answer
on the same socket through `RedisConnection::without_budget`, and end when the
socket's liveness — keepalive, and `TCP_USER_TIMEOUT` on Linux, both at the
budget and never under a second — says Redis is gone rather than slow. Neither is a port call a net sits
over. So every net sits above the budget of
each adapter the framework ships, and a test pins the order wherever both
constants are visible: `nest-rs-redis` asserts its default connect budget is
below `HIT_TIMEOUT` and below the queue port's net. Past a net every member fails
closed, each in its own terms:

- **`OccurrenceLock::claim` / `claimed`** — the occurrence's stale threshold (its
  hold less `MAX_SKEW`), and the loop's cancellation: the occurrence is skipped
  at `warn`, and a claim in flight at shutdown is abandoned.
  **`renew` / `release`** — the renewal's beat, a third of the lease: a renewal
  past it is retried at `warn`, a release past it leaves the lease to lapse, at
  `warn`. A release is not cut by shutdown, because a replica leaving after a run
  is every deploy, and one that kept its lease would hold the job on every peer.
- **`ThrottlerStore::hit`** — `HIT_TIMEOUT`, 20 s beside the trait, and below the
  HTTP edge's request timeout so a hung store reads as the same `429` on every
  edge: the request is denied for the window, at `warn` naming the store
  (`ThrottlerStore::name`).
- **`JobProducer::enqueue` / `remove` / `remove_unique`, and `CheckpointStore::load`
  / `save` / `clear`** — one net the queue port declares for every backend: a
  push or a cancel past it is an error to its caller, and a checkpoint past it
  fails the attempt as retryable, so the job runs again rather than on progress
  nobody confirmed.
- **`Strategy::authenticate`** — `AuthnGuard`'s net, above any shipped strategy's
  budget: the request is denied, fail closed, at `warn` with fields. The shipped
  `JwtStrategy` verifies locally; the net is for an app's strategy that calls a
  backend — token introspection, an API-key lookup.
- **An `#[indicators]` method** — its indicator timeout
  (`NESTRS_HEALTH__INDICATOR_TIMEOUT_MS`) under the probe's deadline: the
  indicator reads down, at `warn`.

**One member is not bounded, and it is the owner's question.** The database:
acquiring a connection is bounded by the pool
(`NESTRS_SEAORM__CONNECT_TIMEOUT_SECS`), but a `Repo` statement — or the `BEGIN` /
`COMMIT` / `ROLLBACK` a job context settles through — on a connection that stopped
answering is not, because `SeaOrmConfig` exposes no statement timeout (sea-orm 2's
`ConnectOptions::statement_timeout`, Postgres's `statement_timeout`). A hung
`COMMIT` holds its tick, and with it the scheduler's stop. Possible, unbuilt, and
raised rather than refused. Outside the family by construction:
`AbilityFactory::define` / `define_visitor`, the WS `Registry` and `ConfigSource`
are synchronous, and an `EventBus` listener is in-process developer code, not a
backend.

**An outbound call the framework makes carries its own bounds**, on the same
net-over-budget reading. `OAuthClient`'s HTTP client has a connect timeout and a
total one, constants argued against the identity-provider calls it makes — the
token exchange and the userinfo fetch, which `nest-rs-social`'s providers make
through it too. `Storage` takes `object_store`'s own: 30 s a request, 5 s to
connect, at most ten retries within three minutes. The OpenTelemetry exporter's
are the SDK's, cited where `nest-rs-opentelemetry` shuts it down. A client
without one is bounded by the edge's request timeout where there is one, and
behind a queue job or a tick by nothing.

**Whether this becomes a `CLAUDE.md` invariant** — *every port call the framework
awaits has a bound, and shutdown abandons an in-flight call rather than awaiting
it* — is an owner question. Until it is answered the rule binds the framework
crates this file is loaded for, and a new awaited call is presumed to owe a net
until it is checked.

**The namespace falls out of the contract — it is never arbitrated.** A port
owns a config namespace **if and only if its contract requires the integrator to
honour a config**. The throttler's does (`resolve(&ThrottlerConfig)` is in the
shared seam), so `NESTRS_THROTTLER__*` has an owner and survives a backend swap.
A delegated port's contract requires nothing, so it owns no namespace: the
adapter's config takes its namespace from where it is declared — read off the
path, exactly as a type name is (*A `#[config]`'s namespace is its stem* in
`architecture.md`). The resource an adapter opens is the crate's own subject, so
its config sits at the crate root and wears the vendor's word — `SeaOrmConfig`
→ `NESTRS_SEAORM__URL`, `RedisConfig` → `NESTRS_REDIS__URL` — and a setting one
binding owns sits in that binding's folder and wears both words
(`RedisWorkerConfig` → `NESTRS_REDIS__WORKER__*`). A `NESTRS_QUEUE__URL` was a
connection filed under whichever binding asked first; a `NESTRS_DATABASE__URL`
was the universal convention naming neither the crate nor the type that parsed
it — both retired for the same reason: from the variable, a reader could not
find the code. Selection *by configuration* is the case where the member is a
folder of the crate, hence two segments: `social__github`, `social__google`.
Selection *by type parameter* is the one case that needs no namespace at all.
Nothing else needs either.

**`nest-rs-storage` is the open breach of this rule, and it is recorded rather
than fixed.** It delegates to `object_store`, whose whole point is being
multi-driver (S3, GCS, Azure, local filesystem, in-memory) — and then pins one:
`Storage` holds a concrete `OnceLock<AmazonS3>`, and every field of
`StorageConfig` is S3's own vocabulary (`endpoint`, `region`, `access_key`,
`secret_key`, `bucket`, `force_path_style`, `allow_http`) under the port's word,
`NESTRS_STORAGE__*`. So a consumer cannot reach GCS or the local filesystem at
all, while **three shipped surfaces say otherwise** — and the two loudest are the
ones a driver author actually opens: the crate's `//!` ("pointing at GCS/Azure/fs
later is a builder change in `Storage`, not an API change for consumers"), the
doc on the `pub` `Storage` type, which renders on docs.rs, and the `description`
in `Cargo.toml`, which is the crates.io landing page: "the GCS, Azure, fs and
memory drivers sit behind the same client." All three are true of *us*, and read
by everyone as true of *them*. That is the sentence this rule forbids: a concern
that is not swappable implying that it is. It is also the naming law's own case —
a bare port name worn by one backend, exactly what `RedisThrottlerModule` exists
not to be. Two ways out and both are the owner's: expose `object_store`'s other
drivers, or rename the crate for the backend it pins. `nest-rs-redis` shows the
second working — it pins `apalis_redis` just as hard, and its name says so.

That derivation replaces three rejected attempts, each recorded so none is
re-proposed: classifying a field as *policy* or *connection* (a judgement no
outsider can repeat), asking whether *a second driver would write it identically*
(a counterfactual whose answer depends on which driver you imagine), and reading
the namespace off the file's path stem (mechanical, but it decides nothing about
where the field should have been declared).

### A transport aggregates; owning a mount is the exception

**A transport aggregates contributions from several providers onto one
mount point.** Controllers mount routes flat into one `Route`; resolvers
merge into one schema; `#[process]` collects per queue name;
`#[scheduled]`, `#[on_event]` and `#[mcp]` the same. **Owning a whole
mount is the exception and has to be justified** — because the moment one
provider owns a mount, a product with two features on that mount has to
fold them into a god-adapter, which inverts the layout
`features.md` mandates. That inversion is always the framework's defect
to fix, never the product's licence to flatten.

Two shapes implement it, and the choice follows from where discovery
lives:

- **Merge a link-time registry** (`inventory`), filtered by
  `ReachableProviders` — GraphQL, queue, schedule, events.
- **Merge container metadata** — MCP. Each `#[mcp]` host attaches an
  `McpHostMeta`; the *first* host on a path also attaches the one
  `HttpEndpointMeta` that mounts them all, so the transport's
  "a mount path is its owner's exclusive namespace" rule stays intact and
  a real collision (an `#[mcp]` beside a `#[gateway]` on one path) still
  fails boot naming both.

Merging introduces exactly one new failure mode, and it must be a **boot**
error naming both owners: two contributions claiming the same addressable
name. For MCP that is a duplicate tool name within a path
(`nest-rs-mcp/src/registry.rs`), because the protocol addresses a tool by
bare name inside an endpoint and the loser would silently be unreachable.

**And it raises exactly one new question: who *is* the mount?** A contribution
answers for itself; the mount's own identity — what a client is told it is
talking to — belongs to the **app**, never to whichever contribution
registered first, or the answer becomes a function of `imports = [..]` order.
So an aggregating surface whose protocol exposes a mount-level identity gives
the app a seam to declare it once (`McpModule::for_root(McpOptions { server, .. })`,
provider-less metadata read back at mount) and lets **at most one** contribution
refine it for its own mount, and:

- **the declaration replaces only what it states** — identity is declared,
  capabilities stay *observed* from the contributions, so an app can never
  advertise a surface nobody implements;
- **a declaration that reaches nothing fails boot**, and two contributions
  declaring one mount fail boot naming both;
- **undeclared is reported, not guessed silently** — a mount left at the
  *SDK's* own default identity is a boot `warn` carrying the remedy. Compare
  against the SDK's own constructor, never a literal, so the check cannot drift
  from the version the framework builds against.

This is the ecosystem's shape, not an invention: one server object created with
its identity, contributions registered onto it (TypeScript SDK), and a parent
that "retains its own name and serves as the orchestrator" when it mounts
children (FastMCP).

**WS is the one justified exception, audited and recorded.**
`#[gateway(path)]` owns its mount: two gateways on one path is the
transport's duplicate-self-mount boot error, and there is no seam to merge
their `#[subscribe_message]` arms. It stays that way because nothing
pushes a product to share a WS path the way MCP's clients push it to share
a URL — a socket per feature costs a client one more connection and
nothing structural, and the thing features actually need from each other
across sockets (fan-out to connections another feature owns) is already
solved by `WsServer<N>` namespaces *without* sharing a mount. So the
adapter shape holds: one `<feature>/ws/gateway.rs` per feature, each on its
own path. If a product ever genuinely needs two features' events on one
socket, that is a framework change on this same pattern (route by event
name, fail boot on a duplicate event) — reported, never worked around.

### A new edge owes the same list — and the list is here

The edge vocabulary is closed (`architecture.md`); the **form** is open, and this
is what the form costs. Every line below is something all four request-carrying
edges do today, with the grep or the test that proves it. Adding an edge means
doing all of it or not shipping the edge — a transport that implements eight of
these is not "a smaller transport", it is a hole a developer discovers at the
worst moment, because the thing it left out is the thing they assumed.

**Read the numbered list as the checklist and the parenthesis as the proof.** A
line whose proof you cannot run is a line you have not done.

1. **Two decorators, one item shape each** — the host on the struct, a sibling
   named for what it collects on the impl. Both halves parse through one
   `DecoratorPair` const (`rg 'DecoratorPair' crates/*-macros/src/` names every
   pair), so the wrong shape is a compile error **naming the sibling**, and each
   pair ships a trybuild snapshot **per** wrong shape.
2. **Mandatory posture per operation** — `#[authorize(Action, Entity)]` or
   `#[public]`, with a trybuild snapshot for the no-posture case. Silence is not
   a posture: the refusal is what keeps *no authn/authz decision outside a guard*
   true, and it is the one item on this list that is load-bearing on its own.

   **The grammar is shared where it is the same grammar, and only there.**
   `PostureRules` in `nest-rs-codegen` words the declaration, the
   mandatory-posture refusal and the `bind = Service` rejection once; `#[tools]`
   and `#[messages]` take it verbatim. The other two parse their own, each for a
   stated reason, and the reasons are the difference — not drift:

   - **`#[operations]` (GraphQL)** accepts `#[authorize(Update, bind = Service)]`
     and `id_arg = ident`, which synthesise an id argument and an
     `Authorized<A, E>` proof. No other edge can express that, and carrying the
     option in the shared rules for two transports that reject it would be the
     abstraction paying for a case it does not have. Argued at the top of
     `nest-rs-codegen/src/posture.rs`.
   - **`#[routes]` (HTTP)** has an **optional** posture — a route's gate may also
     be `#[use_guards]`, which is why `request-layers.md` says the posture is
     mandatory *on the last three* — and two refusals that exist nowhere else:
     `#[authorize]` on an `#[sse]` route, and binding the posture to a handler
     *parameter* (`authorize_param`). A shared `take` returning `Posture` rather
     than `Option<Posture>` cannot serve it.

   So the testable form is per site, not one grep: `PostureRules` is the only
   wording of the two-transport grammar, and each of the four edges ships the
   no-posture (or, for HTTP, the contradiction) trybuild snapshot. **Two of four
   is the correct count here**, and it is written down so the next reader does
   not read a hole where an argument is.
3. **A class gate the posture emits** — `nest_rs_authz::<edge>::authorize`, whose
   *decision* is the shared `gate` so `#[authorize]` cannot come to mean five
   things. Missing ambient ability fails **closed**.

   **HTTP's gate is the one that does not call it, and must not.** The shared
   `gate`'s first rung is `is_visitor()` ⇒ `Unauthenticated`, which refuses every
   anonymous caller before looking at a grant. The sanctioned public-reads
   pattern — `#[public]` beside a hand-written `Authorize<A, E>` — needs a
   `define_visitor` grant to *satisfy* the gate, so `Authorize` open-codes
   `can_class` then `missing_scopes` and has no `Unauthenticated` verdict.
   Argued on `Ability::is_visitor`, which names the split: on HTTP the route's
   own posture asks whether there is a principal, on the in-band edges the gate
   does.
4. **Response masking the same posture arms** — never hand-written at the use
   site, and `unmasked` is the opt-out for a shape the value-level round-trip
   cannot see through. Which of two shapes depends on what the edge does with the
   value: `masked_value_for` when it must reconstruct the return type (GraphQL's
   non-nullable schema, MCP's `structuredContent`), so a stripped required key
   refuses the operation; `masked_reply_for` when it ships JSON (WS), so the key
   is simply absent. **Pick by the protocol, not by symmetry** — and either way,
   fail closed on a missing ambient ability. The witness is a test in
   `nest-rs-authz/tests/integration/<edge>/mask.rs` asserting a field grant strips
   a column **with no masking call in the handler body**.

   **HTTP arms neither function**, and that is a fourth mechanism rather than a
   gap: `#[routes]` installs a `RouteResponseShaper` chosen **by the parameter's
   type** (`ShaperProbe`, so an alias or a re-export arms identically), and the
   shaper omits a masked key from the body. Same fail-closed reading, nothing
   for the expansion to call — which is why that edge's witness proves the
   effect and cannot spell an entry point.
5. **Guards at two scopes** — `#[use_guards]` on the host and per operation, plus
   `#[force_guards]`, composed once per site and deduped by `TypeId`. A denial
   renders through one `denial_to_<edge>_error`, so a guard's refusal and a
   gate's refusal reach the client identically.
6. **A `Guard::check_<edge>` entry** on the trait, feature-gated like its
   siblings — **plus a marker trait, and the bound the decorators emit for it.**
   Every `check_*` defaults to `Ok(())`, so without the bound a guard bound where
   it has no entry passes everything silently. The pattern is `<Edge>Guard: Guard`
   in `nest-rs-guards` with a `#[diagnostic::on_unimplemented]` note, declared by a
   guard beside the `check_*` it attests, and asserted per declared guard through
   `nest_rs_codegen::guard_capability_bounds`. **All four edges assert, HTTP
   included.** The bound never proves a *method* exists — the `Ok(())` default
   guarantees that at every edge — it proves the author **declared** this guard
   checks this edge; an empty `impl Guard for X {}` satisfies the compiler and
   passes everything, and the marker is what turns that into an error at the
   binding site. `HttpGuard` is the one marker carrying no `cfg`: its three
   siblings gate a `check_*` that exists only when that edge is compiled in, and
   HTTP is the substrate the other three mount on, so no build of the crate lacks
   `check_http`. Witness: a trybuild snapshot per edge, binding a guard that does
   not check it at that edge's site. HTTP has **three** emitters —
   `#[controller]`, `#[routes]` and the `#[gateway]` struct, whose guards run on
   the upgrade — and each underlines the decorator the guard was written under,
   with a snapshot of its own (`unattested_guard_on_a_gateway` is the third).
7. **Per-argument pipes** — `Piped<P, T>` / `Valid<T>` stripped by the impl-half
   decorator, rejection rendered as the edge's native error, and the pipe runs
   **after** the gate so a refused caller never pays for validation and a
   validation message never doubles as an existence oracle.
8. **A named compile error for every layer family the edge does not bridge** —
   `reject_http_only_layers`. A silently ignored `#[use_interceptors]` is the
   defect that function exists to prevent; extend it rather than adding a second.
9. **Request scope + a data context** — `Scoped<T>`, and an executor+ability
   re-install per dispatch through `dispatch::with_data_context` so commit and
   rollback semantics cannot drift from the other edges'.
10. **`#[config]` + `for_root`** — one seam, one value, dual-path env.
11. **Error opacity** — an `Opaque` trait beside the edge's error type, whose
    `opaque()` logs the real error at `error` on `nest_rs::<edge>` and substitutes
    `nest_rs_core::OPAQUE_CLIENT_MESSAGE`. **The trait is per edge and only the
    constant is shared**, and that is a finding rather than a preference: the
    trait's output *is* the edge's error type, which is what lets `.opaque()?`
    infer from the enclosing function's return type. One trait generic over the
    output has three applicable impls, the receiver stops deciding, and every call
    site needs a turbofish.
12. **Discovery and its gate** — `Discoverable`, `ReachableProviders` for a
    link-time registry or structural gating for container metadata, and an
    inert-entry `warn` either way.
13. **Aggregation** — several providers at one mount, with a **boot** error naming
    both owners on a duplicate addressable name. Owning a whole mount is the
    exception and has to be argued (WS is the one audited case, above).
14. **A mount** — a `Transport` via `TransportContribution`, or an HTTP self-mount
    declaring its `EdgePosture`.
15. **`nest_rs::<edge>` span target**, level per layer, ≥1 structured field per
    event — **and one `nest_rs::operation` line per unit of work**, through
    `nest_rs_core::operation_log`. The target is a constant from
    `nest_rs_core::target`, never a string; the unit's name is a constant the
    **edge's own crate** declares in its `src/unit.rs` — the kernel holds none,
    for the reason `operation_log`'s module doc gives — spelled `<edge>.<unit>`
    and read by the span, the line's `name:` and its `message` alike; the span's
    kind is a constant from `operation_log::kind`. The `units` join fails on a literal at any of them. A log line renders no span state, so the span's
    attributes say nothing on the console: the line is where the work is named
    (which route, which event, which job, which tool), and an edge without one
    leaves its work anonymous. It carries the edge's own identity fields plus
    `outcome` and `duration_ms`, its field names are **flat** (a dotted name is
    ambiguous to `tracing` beside a path target), and it takes no config toggle —
    the shared target is the family's, so one filter directive silences all of
    them. `duration_ms` is the one field the **text** console pads — to
    `operation_log::DURATION_DECIMALS`, the resolution the formula already rounds
    to — because a duration is read as a column and a width that moves with the
    value is re-parsed by eye every line. JSON keeps the bare number: trailing
    zeros are the reader's affordance, not the machine's.
16. **Four witnesses** — an `integration` suite covering guards / pipes / scope /
    posture; a driver in `nest-rs-testing` if the protocol needs one; an adapter
    in `demo/` (`<feature>/<edge>/`); and a use site in `nest-rs-macro-hygiene`
    proving the decorators need no second manifest line.

Then the packaging: *Shipping a new capability* in `CLAUDE.md` (umbrella feature,
`pub use`, README + docs `## Install`, derive routing, its two witnesses). That
list is about **reaching** the capability; this one is about the capability being
the same shape as its peers once reached.

**Two known asymmetries, both deliberate and both recorded above** rather than
left for a reader to rediscover: a WS gateway owns its mount (audited exception
to *a transport aggregates*), and on GraphQL and MCP the operation *guard*
installs the ambient ability while on WS the *data context* does — because a
gateway is `Guarded`, so its upgrade already ran the real chain and there is
nothing to re-run in band.

**Two residual gaps, and the declaration sites now hold only the smaller one.**
A guard may declare a capability marker without overriding the matching
`check_*` — a deliberate line a reader can see, written next to the method it
should have been. Its larger twin is closed at every *declaration* site: an empty
`impl Guard for X {}` bound by `#[use_guards]` no longer compiles at any of the
four edges, `HttpGuard` being the fourth marker.

**The second gap is the global site.** `use_guards_global([guard::<X>()])` takes
no capability bound (`nest-rs-guards/src/builder.rs`), so an empty guard
registered there still passes everything, silently. Bounding it is not the
answer: a global guard legitimately serves whichever edges it implements, and
requiring `HttpGuard` would refuse a GraphQL-only one — the fix would be a
per-edge global list, four declarations where the developer wrote one.

**"Serves whichever edges it implements" is now true of all four, and was not.**
The pool reaches an operation at the site where the operation exists: HTTP bakes
it into the `RouteShaper`, WS folds it per message, and the two `Exempt`
transports fold it into their per-operation chain — one `compose` in
`dispatch/chain.rs`, no per-transport scope switch. What an `Exempt` endpoint
guard runs is `check_http`, against the request; what the site runs is
`check_graphql` / `check_mcp`, against the operation. **Two questions, so
neither answers the other**, and an edge that ran the first never shortens the
second.

MCP used to say otherwise, and it was a fail-open: `mcp_chain.rs` excluded the
pool from the site, so a global guard overriding only `check_mcp` was never
consulted — while its presence made the pool non-empty and disarmed the deny-all
`is_empty()` tail in `mcp_operation_guard.rs`, so registering it *opened* an
endpoint that refused everything without it. The seam that expressed the false
claim went with it: `McpOperationGuard::already_ran` reported HTTP-scope
execution and was subtracted from an MCP-scope chain, which is a category error
that can only ever suppress a check.

**Two residues, both reported rather than closed, both owner questions.**

- **Discovery is gated where the protocol says it is, and that is not a
  residue.** `initialize` / `tools/list` / `prompts/list` are rmcp server methods
  the endpoint's `check_http` is the only gate on. That was recorded here as a
  hole; the specification says it is the design. MCP "provides authorization
  capabilities **at the transport level**", the server "acts as an OAuth 2.1
  resource server", and "authorization **MUST** be included in **every HTTP
  request** from client to server" — with 401 for an absent or invalid token and
  403 for insufficient scope, both HTTP statuses. Nowhere does the spec
  authorize a JSON-RPC method individually, and it never names `tools/list` as
  needing a check of its own. So the mandated gate is uniform over every HTTP
  request, discovery included, and it is the one this edge runs.

  **The GraphQL comparison does not transfer, and that is the correction.**
  `_service` / `_entities` are gated in band because GraphQL has no
  transport-level authorization to be uniform over — one POST carries an
  arbitrary document, so the field is the only addressable unit. MCP's unit *is*
  the HTTP request. Reading the two as the same shape is what turned a
  conformant edge into a recorded hole. A `check_mcp` chain over discovery would
  be a layer above the standard, not the standard: build it if a product asks,
  and do not carry it here as a debt.
- **The in-band chains are never phase-validated, and only they.** A
  `boot_validate_*` makes a misordered chain — an authorization-phase guard ahead
  of the authentication-phase one whose principal it reads — a named boot failure
  instead of a deployment that denies everything with nothing to say why. It
  fails **closed** (the ability guard finds no principal and installs nothing, so
  `Repo` denies), which is exactly what kept it quiet.

  HTTP has it at the `#[routes]` mount and over the global bucket. **WS has it at
  the upgrade and not per message** — one of its two sites. `#[messages]`
  attaches an `HttpBootCheck` calling `boot_validate_guards` over `#[gateway]`'s
  own emitted specs; that chain is an HTTP `GET`, so it runs `check_http`, which
  is the entry the check is written about. The check lives in the impl half
  while the upgrade's guards are declared on the struct half because a gateway
  freezes its chains **at mount**, with the container in hand.

  **Per message it would be wrong, and this was proved by shipping it.** A
  per-message chain runs `check_ws_message`, while `validate_guard_chain` reads
  `produced_principal` / `expected_principal` — which describe `check_http`, and
  `AuthnGuard` keeps the no-op `check_ws_message` default by design. Applied
  there the check was wrong in both directions at once: silently green on a
  chain where nothing attaches a principal at all, and a false boot failure on
  the split-scope shape `authn-authz.md` sanctions. The phase-*ordering* half
  would transfer; the principal half does not, and making it honest needs a
  per-message notion of what "produces" means. **Owner question**, recorded in
  `guards-baseline.txt` with the two defects that closed it for a day.

  `#[operations]` and `#[tools]` cannot answer there: they compose through
  `SiteChainCell` on the **first dispatch that reaches the site**, after every
  boot check has run, and nothing at boot enumerates the sites. Closing them is
  therefore not a call to add but a link-time registry of sites — the shape
  `GraphqlLoaderRegistration` already has — submitted by both macro crates and
  walked once at boot against `ReachableProviders`. **Owner question**, and the
  fix belongs to both edges at once.

Closing the first gap would mean four `check_*`-carrying traits with no
defaults, and then a guard serving three edges needs three container
registrations — every execution site holds `Arc<dyn Guard>`, and a trait object
cannot be narrowed back. The remedy would cost more than the defect.

**That same arithmetic is why `check_http` stays on `Guard`.** Moving it to an
extension trait is the shape the roadmap reserved a name for, and it buys
nothing: **`nest-rs-guards` itself depends on `nest-rs-http` unconditionally**,
with no `cfg` and no optional flag, so every build that links the guard core
links the HTTP stack whatever the consumer asked for, and a `cfg` on the trait
method saves no bytes. That one manifest line is the whole proof — do not
restate it as a list of dependent crates, which is both longer and false
(`nest-rs`'s bare `guards` feature and `nest-rs-authz`'s `mcp` feature both pull
guards without naming `nest-rs-http` themselves, and half such a list is
dev-dependencies, which no consumer build sees). `cargo tree -i poem` on a
headless feature set names the crates a worker actually pays for — they are
elsewhere, and each is its own report.

### Lifecycle hooks

`#[hooks]` submits phase-tagged methods (`#[on_module_init]`,
`#[on_application_bootstrap]`, `#[on_module_destroy]`, …) to `inventory`;
`App::run` drains per phase. Per-provider, run in `(provider, method)`
name order; init failure — an error or a panic — aborts boot with the hook
named. Shutdown is best-effort — a hook that fails or panics is logged at `error`
and the next one runs — **and bounded**: the three shutdown phases share one
budget declared beside the phase runner and argued there, and a hook still
waiting when it is spent is abandoned with a `warn` naming its module and the
hook. Every later hook still starts and is polled once, so none is skipped in
silence. A hook is developer code, so a panic in one is contained where the
runner awaits it, as a transport's is by its `JoinSet`.

### Shutdown is bounded end to end

**A wait on the way down without a bound is a `SIGKILL` with nothing to say
why.** An orchestrator sends `SIGTERM`, waits its grace period — 30 s by default
on Kubernetes — and kills: whatever the process still waited on dies with it, and
a replica that never exits stalls a rollout. So every wait on the way down has a
bound, and what still runs at the bound is abandoned with a `warn` naming it,
never awaited in silence. The way down is three steps, in order, each with its
own bound:

1. **The transports stop, together.** The signal cancels the token every `serve`
   shares, and `App::run` joins them all.
   - **HTTP** hands poem a graceful-shutdown timeout, `HttpConfig::shutdown_timeout`
     (`NESTRS_HTTP__SHUTDOWN_TIMEOUT_SECS`, pinned or from the environment: 20 s by
     default; 1 s to an hour). A connection still open at the bound — a streaming
     body such as an SSE stream or an MCP session — is closed, with one `warn`
     saying how many were, and a handler on it is dropped, over HTTP/1.1 and
     HTTP/2 alike, filing its `http.request` line `outcome = cancelled` with no
     `status` (a stream's head was answered, so a cut stream files its line with
     that status and the bytes written, as its body ends). The request timeout
     bounds a handler, never a streaming body, which is why this bound is the
     transport's own.
   - **Work a connection only carries stops with the transport.** A self-mount
     that runs its units off the connection that asked for them — rmcp runs each
     MCP operation on a task of its own — declares an
     `HttpEndpointMeta::runs_detached(DetachedWork)`, and `serve` stops it as the
     last thing it does, waiting `DetachedWork::SETTLE_TIMEOUT` (500 ms) at most
     for it to unwind. Without it a cut connection left its operation running
     through the shutdown hooks. A stopped unit files `outcome = cancelled`.
   - **The Redis worker** stops fetching and drains within
     `RedisWorkerConfig::shutdown_timeout` (*A shutdown stays inside
     `shutdown_timeout`*, in the queue's entry below). An attempt the drain
     interrupts files its `queue.job` line `outcome = cancelled` — from the port,
     which files it for any attempt a driver drops, so a second adapter owes
     nothing for it.

   **A unit of work cut on the way down still files its operation line**, with
   `outcome = cancelled` and the time it ran: *every edge files one line per unit
   of work* holds for the units a shutdown stops, which are exactly the ones an
   operator reads the log for afterwards. Where the edge does not stop the unit
   itself — HTTP's handler is dropped by hyper, a queue attempt by its driver —
   the line is filed by a guard dropped with the unit's future, so it cannot
   depend on the edge noticing the stop; MCP stops its operations itself and
   files the line where it does.
   Built at HTTP, MCP and the queue; a WebSocket or GraphQL-over-WS handler is
   not closed by the window at all, which is an owner question.
   - **The scheduler** starts no tick once shutdown is observed and abandons a lock
     call in flight, but it **joins a tick already running** rather than dropping
     it, because a dropped attempt holds its row locks until its statement drains
     (`data-layer.md`, *An abandoned attempt*). A tick that never returns therefore
     holds the stop, and bounding a running tick — and deciding what it would
     release — is an owner question.
2. **The shutdown hooks run** — `#[on_module_destroy]`,
   `#[before_application_shutdown]`, `#[on_application_shutdown]` — all three
   phases inside **one** budget, `SHUTDOWN_HOOKS_TIMEOUT` (5 s): a deadline they
   share, never a bound per hook, since `k` stuck hooks under a per-hook bound
   cost `k` times it and no grace period can be sized against that. A hook that
   panics is contained and named at `error`, like one that fails.
3. **Telemetry flushes** when `main` drops the guard `OpenTelemetry::init_with`
   returned: the tracer, meter and logger providers shut down **concurrently**,
   each on a thread of its own, held to `nest_rs_opentelemetry::FLUSH_TIMEOUT`
   (3 s) between them; what still exports then is abandoned and named on stderr.
   The SDK's own bound is five seconds per provider, in turn, and its metrics
   provider ignores the timeout it is handed, so the bound is the crate's.

**The steps add up, so the grace period has to hold their sum** — the longest
transport bound, then the hooks' budget, then the flush: 20 + 5 + 3 = 28 s by
default, two under the kubelet's 30. `the_default_shutdown_steps_sum_under_a_kubernetes_grace_period`
in `nest-rs-testing` reads the three constants and fails the day they stop
fitting. That is also why the demo chart gives the worker 45 s for its 30 s
drain.

## Surface crates — decisions, not mechanics

- **`nest-rs-http`** — the only activation seam is
  `HttpModule::for_root(...)` in imports; no public `.transport(...)`.
  Every `HttpConfig` field settable via `NESTRS_HTTP__*` env **and** the
  pinned struct — the framework-wide **dual-path config rule**, which
  applies to every `nest-rs-*` module.

  **`#[sse]` is a verb, not an edge.** It sits beside `#[get]`/`#[post]` in
  `#[routes]`, collapses to `GET` before the route table is built (so
  `#[sse("/x")]` beside `#[get("/x")]` is the ordinary duplicate-route error),
  and owes none of *A new edge owes the same list* — it carries the edge's
  guards, pipes, posture and document verbatim. What it owns is the response:
  the handler returns an `SseStream`, the decorator writes the
  `text/event-stream` and arms the ceiling. **Four** refusals, each a named
  compile error, and they are named rather than counted because the count is
  what drifted: `#[authorize]` (masking has no wire model to reconcile against —
  a capability-only guard is the pattern), a **shaper parameter** — a
  hand-written `Authorize<A, E>` or a `Bind<A, S>`, which is the second and
  less obvious way to the same place, and on a stream the worse one, since the
  mask waves an opaque body through while the document says it masked — the
  response-decorator family in one sentence (`#[http_code]` / `#[redirect]` /
  `#[response_header]` all shape a response that *completes*), and
  `#[api(response_content_type)]`.

  **Correlation is not among what it owns, and that is the point.** An SSE
  stream is polled after the handler returned, so its events once filed under no
  request at all — closed at the *body*, in `response_body.rs`, which wraps every
  streaming response the transport serves. `#[sse]` is the loudest member of that
  family and was never the scope of the fix: a hand-built `Body::from_bytes_stream`
  is the same gap, and closing it at the decorator would have left the other one
  open. See *Correlation* in `CLAUDE.md`.

  **The stream ceiling lives in `HttpConfig`, and the namespace is the whole
  argument.** `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS` is the third instance of
  one security control — a long-lived connection authenticates once and then
  replays those privileges — so it takes its peers' reading, default and `0` ⇒
  unlimited spelling verbatim. It does **not** take their namespace: an `sse`
  one would mean an `SseConfig`, which under *one seam per config* would owe a
  `for_root`, which would mean a module for a response shape. SSE is not a
  module, so the knob belongs to the transport that serves it. Asymmetry
  argued, not silent.
- **`nest-rs-pipes`** — transport-agnostic, **one Pipe per file**,
  stateless (`transform(In) -> Result<Out, _>`, never a DI provider).
  Binds **per argument on all five transports**, two forms by design
  (orphan rule): HTTP wraps an extractor (`nest_rs_http::Piped<P, E>` /
  `Valid<E>`); GraphQL, WS, MCP and queue wrap the wire value
  (`nest_rs_pipes::Piped<P, T>` / `Valid<T>`, stripped by
  `#[operations]`/`#[messages]`/`#[tools]`/`#[processor]` — on MCP the carrier
  goes *inside* `Parameters<…>`, which is what the protocol deserializes an
  operation's arguments into). A rejection surfaces as the transport's native
  error (400 / GraphQL error / WS error frame / `invalid_params` / job
  error). Global pipes exist on HTTP only. **Reusable pipes are
  framework primitives — never define one in an app.**
- **`nest-rs-schedule`** — `#[scheduled]` orchestrator; methods tagged
  with exactly one of `#[every]` / `#[cron]` (optional `tz`) /
  `#[after]`. Literals validated at compile time — **`tz` included**: it is
  always a string literal over the closed IANA name set, so both of `#[cron]`'s
  keys are refused at the line that wrote them rather than one of them at the
  boot. `CronExpression` presets are paths, so they are the one thing that still
  resolves at boot. `Scheduler` is a `Transport` via `TransportContribution`.

  **Where a recurring job fires is declared, never inferred.** `#[every]` and
  `#[cron]` take `replicas = "each"` (the default: every replica fires) or
  `"one"` — **as if one replica ran the job**: each occurrence fires on at most
  one replica, and no two runs overlap, since one replica never overlaps its own;
  `#[after]` refuses the key, since a one-shot fires on the replica that booted.
  The first half is the occurrence's *claim*, the second the job's *run lease*,
  taken atomically with it and renewed while the run lasts: an occurrence falling
  due while the job runs elsewhere is left unclaimed, and the replica running it
  fires the latest late and reports the rest, as one replica does. A job's
  identity — what its lease and its claims are keyed on — opens with the path of
  the module that declared it, because the lock is shared by every app of a
  deployment and the boot sees one: two apps' same-named jobs are two jobs, one
  job in a crate both link is one. `"one"` claims each
  occurrence through the `OccurrenceLock` port, selected by import —
  `nest_rs::redis::RedisScheduleModule` (feature `redis-schedule`) binds it as a
  declared factory carrying `nest_rs_schedule::BACKEND_REMEDY` — so a reachable
  `"one"` job with no binding fails the boot naming the job, and two bindings
  contest. **At most once, never at least once**: a claim that errors, one still
  unanswered when its occurrence goes stale or when shutdown is asked for, and an
  occurrence reached that late are all skipped at `warn`, and a replica that stops
  after claiming loses the occurrence and holds the job until its lease lapses. A claim answered before shutdown is
  observed still fires, as a tick that won its wait does — withholding it would
  lose an occurrence this replica holds the key to, which no other replica can
  then fire. Work that must not be lost is a queue job the tick pushes. The
  lock's backends: Redis, built; an in-process one, refused — a lock no other
  replica can see decides nothing across replicas, which is why `BACKEND_REMEDY`
  answers with `replicas = "each"`; a database one (an expiring claims table, or
  an advisory lock) is possible and unbuilt — an owner question.
- **`nest-rs-queue` + `nest-rs-redis`** — backend-agnostic queue contract
  (`Job`/`ProcessMethod` + `#[processor]` + inventory seam, the `JobProducer` and
  `CheckpointStore` seams, and the capabilities a `QueueBackend` declares) with
  Redis first-class, on apalis-redis 0.7.4 — kept past the freshness bar at its
  pin (`manifests-ci.md`). The adapter crate is named for the **storage** (Redis),
  because that is the surface a caller touches; apalis is hidden — **no apalis
  types leak**. Queues identified by name (a `#[queue]` marker type, or a string
  through the raw hatch). Producer/consumer decoupled. One connection, opened by
  `RedisModule::for_root` (`NESTRS_REDIS__*`) and shared by every Redis binding;
  the producer binds through `RedisQueueModule` (bare), the consumer activates
  via `RedisWorkerModule::for_root` (`NESTRS_REDIS__WORKER__*`; producer-only
  apps skip it), and each binding factory that reads the connection declares it
  runs *after* the connection's, so `imports` order stays a readability choice.
  `throttler` and `schedule` are the crate's features (`redis-throttler`,
  `redis-schedule` on the umbrella) because each pulls a port crate that an app
  which never rate-limits, or never fires a job once across replicas, has no
  other reason to compile; the queue and worker bindings are what `redis` is for.

  **`RedisConnection` is the connection**, not a pool or a factory of them: one
  multiplexed `ConnectionManager`, handed to apalis as the connection its storage
  runs on and used as it is by the rate limiter and the schedule lock. **The boot
  proves it with a `PING`** — a Redis that accepts the dial and answers nothing
  would otherwise boot cleanly and fail on the first job — and **what fails the
  same way every time fails at once**: a zero budget, a URL the client cannot
  parse, TLS settings it will not use, and an answer naming the deployment's own
  settings — refused credentials, an ACL denying the proof, a database index out
  of range, a protocol the server lacks. **That list is an allow-list, and every
  other answer is retried**: `redis` marks every code it does not know as not
  worth retrying, which failed the boot in milliseconds on a Redis busy running
  a script, telling the operator to check the URL. What may clear — a refused or
  reset TCP connection, `LOADING`, `BUSY`, `MASTERDOWN`, `TRYAGAIN`, a code this
  client has never seen — is retried within `connect_timeout`, then fails naming
  the endpoint, never the URL, which may carry a password: as `Unready` with
  Redis's last answer as the source when Redis answered, as `Unreachable` when
  it did not. The proof's own connection is closed before the kept one opens, so
  a Redis with one client slot left boots. Every
  command a caller waits on afterwards answers or fails within that budget, end
  to end — the wait for a reopened connection included — so an outage fails a
  command instead of holding every loop. **Certificate verification is never an
  option**: `rediss://…#insecure` fails the boot and `redis` is built without
  `tls-rustls-insecure`, because a private authority is trusted by configuring
  its certificate, not by skipping the check; TLS material beside a plaintext URL
  fails the boot too, since it would go silently unused. **Redis Cluster is
  unsupported** — apalis 0.7's scripts touch keys across hash slots — and the
  queue pages say so. The oldest Redis the docs claim is the oldest the Redis e2e
  suite passed on at the release (6.2.20 for 7.0, beside 7.0.15 and 8.6.3), never
  a version the suite has not run.

  **The port owns the attempt; the adapter owns the transport.** What a job
  attempt *is* — opening the envelope, continuing or minting the trace, the
  `queue.job` span and the ambient scope, catching a panic, classifying the
  outcome into ok / retry / dead-letter / defer within the method's retry budget and
  timing the wait before a retry, the events saying why an attempt failed and the
  `nest_rs::operation` line — is `nest_rs_queue::consume::attempt`, written once
  and tested once in the port. An adapter's consumer is a fetch loop that builds a
  `Delivery` per job, calls it, and translates the `AttemptOutcome` into its
  backend's vocabulary (apalis's `Abort`, or a re-filing; a NATS consumer's `ack`
  / `nak` / `term`); discovery of the `#[process]` methods, module-gated and
  refusing every declaration the backend's `QueueBackend` does not declare, is
  `nest_rs_queue::consume::discover`, and a push option the backend lacks is
  refused by the port before the backend sees it. A second adapter therefore
  copies nothing — and an adapter that opens a `queue.job` span of its own, or
  keeps a retry budget of its own, has taken semantics it does not own.

  **A newer wire version is handed back, never dead-lettered.** An older worker
  meeting an envelope a newer release sealed cannot read it and cannot re-seal
  it, so `attempt` answers `AttemptOutcome::Defer` — no attempt spent, the record
  re-filed as stored, due again after `NEWER_RELEASE_WAIT` — and warns once per
  delivery naming both versions, under the id the newer envelope spells when it
  spells it as this release does. A rolling deploy, where old replicas meet new
  producers for minutes, must not lose a job; an older version is still refused,
  since reading an older shape is the bumping release's decision. The outcome
  enum is exhaustive on purpose, so a variant added later is a compile error in
  every driver rather than a job one of them drops.

  **A job is named by the port.** The push mints a `JobId` — a UUID v7 — and
  seals it in the envelope, and that id keys everything kept about the job: its
  receipt, its span's `messaging.message.id`, a cancel, a unique claim, a
  checkpoint, the delivery guard. A backend's own id for the record reaches a
  job's lines only as `backend_id`. The retry budget (`retries = N`) and the wait
  before each retry are the port's too: exponential from one second to five
  minutes, jittered by a hash of the job's id and the attempt rather than a random
  draw, so every wait is reproducible — in a test, and by an operator reading a
  job's lines.

  **`#[process(concurrency = N)]` is per method, per replica — the vertical bound
  — and replicas are the horizontal one.** A method runs at most `N` attempts at
  once on one replica (default 1), from a permit pool of its own, so another
  method's jobs never wait on it; throughput beyond that comes from replicas,
  which a queue-depth autoscaler adds — KEDA's `redis` list trigger on
  `nestrs:queue:<queue>:active`, the list apalis fetches from, rather than CPU,
  which an I/O-bound worker leaves flat whatever its backlog. Not a capability: any
  backend bounds in-process parallelism, so every backend owes it. **This
  reverses the 1.2.0 decision of `6f787ce5`**, which removed the key because it
  capped nothing — it only sized apalis's read buffer — and left scale to replicas
  alone. The key now bounds what its name says, and the reason for the removal
  went with the defect.

  **The fetch is apalis's, and its ceiling is stated rather than hidden.**
  apalis-redis 0.7.4 fetches up to `buffer_size` records once per
  `poll_interval`, only while the worker has a free permit, and keeps
  `fetch_next` private, so those two settings are the only levers short of a
  fork. The worker sets `buffer_size` to the method's `concurrency` — a buffer of
  one held every method to one job per poll whatever it declared, 9.3 jobs a
  second at concurrency 1 and 4 alike — **and 799 at the most**, a cap derived
  rather than chosen: apalis's scripts hand a fetch's ids to Redis in one Lua
  `unpack`, which stops at 7,999 values, and its sweep of a silent peer moves ten
  fetches' worth in one script, so with a larger buffer a sweep can pop more of a
  dead replica's jobs than it can file back, and lose them (`MOST_PER_FETCH`; the
  limit is pinned by an e2e test on every Redis the matrix runs). `poll_interval` comes from
  `RedisWorkerConfig` (`NESTRS_REDIS__WORKER__POLL_INTERVAL_MS`, pinned or from
  the environment, default 100): at least 10 ms, and at most the orphan threshold,
  because apalis sweeps silent peers on the poll and a longer one leaves a crashed
  replica's jobs waiting past it — each refused naming the variable. Three
  consequences are documented where the queue's scaling is: a method's ceiling
  per replica is `concurrency / poll_interval` for short jobs; a saturated worker
  holds at most `concurrency` fetched records beyond the ones it runs, which no
  other replica can take meanwhile; and every poll costs Redis a fetch and an
  orphan sweep per method per replica whether or not a job waits — the idle price
  a shorter interval multiplies. A higher ceiling is apalis 1.0's question for the
  owner, never a fork, and so is **a fetch sized per method** — a `prefetch` key,
  or fetching only the permits that are free — since a replica running long jobs
  holds fetched jobs an idle peer cannot take and KEDA does not count: possible,
  unbuilt, and an owner question until it is decided.

  **Delivery is at least once, and a redelivery runs once.** apalis delivers a
  job twice in ways no setting of its public API removes — its startup sweep
  reclaims every registered consumer's in-flight jobs, a live peer's included; a
  replica that misses its heartbeats is swept; an acknowledgement is lost in a
  drain — so the worker guards every delivery in keys of its own (*A key a
  datastore holds*, above). An attempt runs only under the job's lease (`SET NX
  PX`, renewed every third of the lease while it runs); its terminal outcome
  writes the settled mark and drops the lease in one script; a delivery arriving
  while the lease is held is **handed back, never acknowledged**, for when that
  lease would lapse; and one arriving after the job settled is acknowledged
  without running, answered as the first was. **The guard may delay a job, never
  lose one**, and each of its steps is one Lua script or one command. Each replica
  consumes under an apalis worker id of its own — the host, then a UUID v7 — and
  the periodic sweep takes a peer's jobs only once it has missed its heartbeats
  for `orphan_after` (ten of them, by default): only apalis's startup sweep uses
  *now*, and the guard is what makes that one harmless.

  **apalis never retries, and never ends a job, on its own.** A dead letter is
  apalis's `Abort`; a `Retry { after }` re-files the same apalis task, carrying
  the next attempt and due once `after` has passed, then takes it out of flight —
  schedule first, out of flight second, so no failure between the two loses the
  job — and a held lease, a throttle window and a shutdown hand a job back the
  same way. A retry is therefore a filing on the schedule, never a wait holding a
  permit or a shutdown — the port's rule for a backend declaring `DelayedPush`,
  which this one does. apalis also counts every delivery, and kills a record
  answered with a plain error once that count reaches its context's private
  `max_attempts`, 5 by default: the adapter answers so only where a hand-back
  failed, but retries, throttle deferrals and lease hand-backs all count toward
  it, and no port event fires. So every record the adapter files — push, delayed
  push, re-filing, hand-back — carries a context whose `max_attempts` is
  `u32::MAX` (`LIFTED_CAP`), built through `RedisContext`'s public `Deserialize`,
  and a unit test pins apalis's serde field names, so a bump that renames them
  fails the tests, not a deployment. **`u32::MAX`, never `usize::MAX`**: apalis
  reads the cap into a `usize` as wide as the host reading the record, so a cap a
  64-bit replica wrote would not decode on a 32-bit one, and apalis's fetch would
  fail every poll for the batch holding it. No job is delivered four billion
  times, and a test decodes the stored cap as 32 bits wide.

  **A shutdown stays inside `shutdown_timeout`.** The worker stops fetching at
  once, lets running attempts finish for the window less a reserve (five seconds,
  or half the window), then interrupts what still runs and hands each job back,
  due at once for another replica, within the reserve; anything still running
  past the whole window is said at `error` and left to its lease. The drain is the
  worker's own rather than apalis's, because apalis-redis 0.7.4 drops the
  acknowledgement of a task that ends while its worker drains: a hand-back takes
  the task out of flight itself, and a job settled during a drain — or within
  the span an acknowledgement takes before it — keeps its settled mark for a
  week, since whichever replica starts next delivers it again.
  What an interrupted attempt's transaction holds is `data-layer.md`'s, under *An
  abandoned attempt*.

  **The Redis backend declares all five capabilities**, each kept in keys of its
  own and proved by its own e2e, and a capability joins the declaration one at a
  time — never as `Capabilities::ALL`, so one the port names later is not claimed
  before it is honoured. The decisions each carries: a delayed record is promoted
  by the producer that filed it, on a one-second tick while it has delayed
  records outstanding, so a due job reaches `…:active` — the list KEDA reads —
  with no worker running, and a deployment with no producer process running keeps
  `minReplicaCount: 1`; **a worker promotes its own queue's due records at the
  fetch's pace** — on its own one-second tick, through the same public
  `enqueue_scheduled`, in batches of `min(799, max(100, concurrency))` while any
  are due, the producer's shape at the worker's size — where apalis's own scan
  moved one fetch's worth a second, a single job for a method of concurrency 1; so
  a burst of retries or hand-backs falling due together reaches `…:active`, and
  the autoscaler, as fast as a replica can take it, and its Redis cost is written
  where the queue's scaling is; a unique key is claimed atomically before the job
  is filed
  and released when the job settles or is cancelled — **at most once over pushes,
  never a lock**; a cancel writes its tombstone only while no attempt holds the
  lease, so `Ok(true)` means the job never starts, and apalis's structures are
  never touched; a throttle is a fixed window per queue, opened by its first start and
  closed by its key's expiry — the HTTP limiter's algorithm, so no two replicas
  have to agree on a clock — and starts either side of a window's end can reach
  twice the limit in a short span, which the page says; **a refusal shuts that
  replica's fetch for the method until the window ends** (`worker/gate.rs`, a
  tower layer holding the worker unready, so apalis fetches nothing), because a
  throttled backlog left fetchable is admitted, refused and re-filed every poll —
  a window costing the backlog, never `limit`; with the gate it costs each
  replica one fetch of refusals; a checkpoint is one key per job, cleared at its
  terminal outcome.

  **One queue per runtime key is not offered on this backend, not deferred.**
  `#[queue]` takes `name` and `job` and nothing else — `prefix` never shipped in a
  release, so it gets the unknown-key refusal every other stray key gets, and the
  reason lives here: apalis 0.7 binds one
  storage to one namespace and one worker to one storage, an idle worker still
  polls Redis on every interval — about 58 commands a second each at the default
  interval, measured, so a thousand tenants would cost Redis some 58 000 a second
  before any job existed — and KEDA's `redis` trigger reads one list, while every
  instance would fill its own. The key rides in the job. A backend whose consumer
  reads many keys through one fetch, with one aggregate signal for an autoscaler
  — Redis streams with consumer groups, a later apalis — could offer it: an owner
  question, which is why this paragraph names the Redis backend's facts rather
  than an impossibility.

  **`#[input]` stays re-exported at the queue edge and stays off the queue
  scaffolds — both on purpose.** Unknown-key rejection is the right default
  where the sender is an untrusted caller; a job payload's sender is the
  producer, possibly one deploy ahead, so the same rejection dead-letters
  the job on attempt 1 instead of ignoring the field the worker does not
  know yet — retries never help, the payload never changes. The scaffold
  therefore writes tolerant serde derives; a payload that wants *value*
  validation may still opt into `#[input]`, accepting that its producer and
  workers now version together. Asymmetry argued, not silent.
- **`nest-rs-ws`** — **not a `Transport`**: the WS upgrade is an HTTP
  GET, so `#[gateway(path = "/ws")]` self-mounts on `HttpTransport`
  (inheriting port/CORS/TLS). `#[messages]` orchestrates
  `#[subscribe_message]` + `#[on_connect]`/`#[on_disconnect]`; one
  envelope `{event, data}`. Per-gateway namespace via `WsServer<N>`.
  A gateway **owns** its mount — the audited exception to *a transport
  aggregates*, recorded above; sharing state across gateways is what
  `WsServer<N>` is for, not sharing a path.

  **A handler returns at most a `Result` around a `Result`-free value.** The
  reply is decided by type (`ReplyValue`), so an alias is a `Result` like the
  literal, and the value is split twice: an `Err` at either level is an error
  frame and a `warn`. A `Result` inside an `Option`, a `Vec` or a struct is data
  and is serialized as such — the frame carries `{"Err": …}` because the handler
  said so. An `Err` becomes a frame through `ErrorReport`'s three tiers — a
  `Send + Sync` error, any other error, `Display` — so a cause chain is logged
  whenever the type has one.
- **`nest-rs-mcp`** — also not a `Transport`, also an HTTP self-mount, but
  it **aggregates**: several `#[mcp]` hosts merge into one `CompositeHandler`
  behind one endpoint, because MCP namespaces tools per endpoint and clients
  point at a single URL. One host on a path is served verbatim; the merge only
  engages beyond that. A host contributes an `McpHost` (the object-safe
  `ServerHandler` view) — it never has to know it is sharing. Guard,
  `dyn McpToolContext` and `McpConfig` are container bindings, so they resolve
  **once per path**, not per host.

  **A host's `path` is a join key, not a namespace.** Unlike a
  `#[controller]`'s, nothing nests under it: it names the one endpoint the host
  joins, which is why peers writing the same path share it. So it is written
  whole — the URL a client config carries — and `DEFAULT_PATH` (`/mcp`) is what
  a bare `#[mcp]` takes. It is a **constant, not config**: a path a *decorator*
  declares is code everywhere here (`#[controller]`, `#[gateway]`), and
  `HttpConfig.global_prefix` already moves the whole surface. (A module that
  owns its whole mount *may* configure it — `NESTRS_GRAPHQL__PATH` does — which
  is why `HttpEndpointMeta::new` normalizes every path it is handed rather than
  trusting the caller.) A prefix was tried and removed — a prefix prefixes a
  namespace, and there is none here.

  **A host writes one decorated `impl`, carrying the same request layers every
  other edge has.** `#[mcp]` on the struct declares the host and takes
  `#[use_guards(...)]`; `#[tools]` on its inherent impl declares the `#[tool]` /
  `#[prompt]` operations, absorbing rmcp's routers, handler attributes and
  `get_info`, and deriving the advertised capabilities from the roles present.
  Each operation takes `#[use_guards]`/`#[force_guards]`, a **mandatory**
  posture (`#[authorize(Action, Entity)]` / `#[public]`) and per-argument pipes.
  The expansion emits a delegating wrapper carrying `#[tool(name = …)]` rather
  than rewriting the authored body, so the developer's method keeps its real
  signature and the wire name stays the authored one. A description is
  **`#[tool(description = "…")]`** — the declared form, because the sentence a
  model reads is behaviour, not commentary, and a workspace that carries no
  comments must still be able to state it. A doc comment is the *fallback* for a
  codebase that does write them, so the prose is never authored twice; an
  operation with neither **does not compile**. A host serving a
  hand-written `ServerHandler` surface (resources, completion) stays on rmcp's
  raw shape; see *Macros* above.

  **A failing operation talks to a language model.** `Opaque::opaque` is the
  framework's seam for that: the real error is logged at `error` on
  `nest_rs::mcp`, the model gets `nest_rs_core::OPAQUE_CLIENT_MESSAGE`. Never hand
  a `Display` straight to a tool's caller — a `DbErr` carries schema, columns and
  sometimes values. A deliberate `McpError::invalid_params` is the opposite case
  and is returned directly.

  **The reasoning was never MCP's**, and the seam is no longer either: a GraphQL
  error frame and a WS error frame are read by clients just as untrusted, so
  `nest_rs_graphql::Opaque` and `nest_rs_ws::Opaque` are the same trait beside
  their own error type. Three traits rather than one generic over the output, for
  the inference reason under item 11 of *A new edge owes the same list*.

  **Identity has two owners, and neither can shadow the other.** One endpoint
  reports one `serverInfo` and one `instructions` however many features share
  it, so: the **app** declares itself once (`McpOptions { server }`) — `name`,
  `version`, branding *and* `instructions`, because a feature library knows
  neither the binary's version nor, on a shared endpoint, the whole surface —
  and a **host** declares only which endpoint stands apart
  (`#[mcp(name = …, title = …)]`, optional, overriding the app's per field).
  `instructions` is deliberately not a `#[mcp]` argument; a host writing one is
  a compile error, and per-tool prose belongs to `#[tool(description = …)]`.
  Two hosts declaring one path fails boot naming both. Identity is **not**
  config — it has no `NESTRS_MCP__*` twin, which is why it travels in
  `McpOptions` beside the config rather than through a second call.
- **`nest-rs-openapi`** — import `OpenApiModule`; self-mounts
  `GET /api-json` + offline Swagger UI at `GET /api`. Document
  **composed** from the route table. Schemas via **schemars**;
  `#[api(...)]` enriches an op.
- **`nest-rs-social`** — open provider contract. **Flow-owning**
  `SocialProvider` trait: `authorize`/`exchange` default to the shared
  PKCE/CSRF flow (through the provider's `nest_rs_oauth_client::OAuthClient`,
  whose `exchange` yields a `TokenSet`), so a standard provider implements
  only `profile`; a non-standard one (Apple's ES256 secret, id_token
  identity) overrides a step **without changing the trait**. A social
  provider is **not a DI provider** — never `#[inject]`ed by type, only
  reached through `SocialRegistry` as `Arc<dyn SocialProvider>` — so it
  has **no per-provider module**: `SocialModule` owns every entry and is
  the single import. Within that gate, credentials decide: complete ⇒
  active, absent ⇒ inert + `warn`, partial/invalid ⇒ boot fails naming
  the provider. A duplicate key, or a registry key disagreeing with the
  provider's own `key()`, **fails boot**. **`SocialModule` takes no
  configuration** — the entry names the provider's own `#[config]`, so
  discovering a provider is what loads its credentials, and the module, which
  never learns the provider exists, has nothing to declare. See the converse
  corollary under *`for_root` — one seam, one value, no chain*.
  A third-party provider crate is therefore exactly two
  files: `config.rs` (`#[config]` + `SocialProviderConfig`) and
  `provider.rs` (`SocialProvider` + `inventory::submit!` whose `build` is
  one `resolve_provider` call). Ships first-party GitHub + Google;
  third-party provider crates are **encouraged** through the same public
  seam. Keyed injection (`#[inject(key)]`) stays the tool for **static,
  compile-time roles** (primary/replica pools).

  **This extension-crate posture — a public behavioral contract +
  inventory discovery — is the template for any future open-ended
  library in the repo.**
