---
paths:
  - "crates/*-macros/**"
  - "crates/nest-rs-codegen/**"
---

# Decorators — macros and their shared grammar

Decorators are the framework's leverage (`CLAUDE.md`, *Identity*). The pair rule
and the umbrella's front door are stated there, the expansion witness in
`manifests-ci.md`; this file is how a decorator is written so that they hold.

## Where a decorator lives

A `proc-macro` crate can export only macros, so each decorator lives in a
companion `*-macros` crate that its surface crate re-exports. Its `lib.rs` holds
the `#[proc_macro_attribute]` items — the one licensed exception to "`lib.rs`
carries no logic", since Rust forces them to the crate root — as thin
delegations into the crate's own modules. What several macro crates share lives
in `nest-rs-codegen`: its root items are the kit a third-party macro crate
builds on, which semver covers (`manifests-ci.md`, *Semver*), and the rest is
its `__private` (`decisions/codegen-kit.md`). A `*-macros` crate never depends
on its surface crate.

## Every emitted path is rooted

An expansion lands in the developer's crate, so it emits only `::std` /
`::core` paths and `::nest_rs_<concern>::` roots, which
`nest_rs_codegen::reroot` — called by every decorator on its finished
expansion — rewrites to `::nest_rs::<concern>::` at a call site holding the
umbrella; a framework crate, which cannot depend on the umbrella, keeps the
sibling root. A macro crate roots a sibling concern only when the umbrella
feature of its own capability enables that concern (`http` enables `guards`
and `pipes`, so `#[routes]` roots both), or when an attribute the developer
writes drives the emission and the concern is attribute-gated in codegen's
crate table (`#[authorize]` reaches `authz`, `#[crud]` `seaorm`,
`#[wire_enum(graphql)]` `graphql`): `reroot` follows that path with the
umbrella's check for its feature, under the `cfg`s around it. Never a bare third-party path — `::anyhow`, `::tracing`, `::uuid`
resolve against the consumer's extern prelude and break every app that lacks
that direct dependency. Routing through the umbrella is also what dissolves
the cycles: `nest-rs-guards` and `nest-rs-pipes` sit below the transports, on
core alone, `nest-rs-authz` and `nest-rs-seaorm` above them, and only
`::nest_rs::` above all of them. Held by `nest-rs-macro-hygiene`'s
per-feature check.

**A seam that is not public API lives in its crate's `__private`** — one inline
`#[doc(hidden)] pub mod __private` of `pub use` in `lib.rs` — reached as
`::nest_rs_<x>::__private::…`. What Rust cannot place in a module (a trait item,
an exported macro) takes a `__` prefix and stays hidden; a hidden method becomes
a free function in the type's own file; an item in a public module sits in that
file's `pub(crate) mod __private`, so no other path reaches it. What an
expansion emits on the developer's type and calls there — its constructors
`__nestrs_from_container` and `__nestrs_from_scope`, the `__nestrs_injected*`
helpers — is `#[doc(hidden)]` and `__nestrs_`-prefixed: the macro ABI, which
no developer and no framework crate calls by hand. Everything else
is API or `pub(crate)` (`decisions/visibility-tiers.md`). A public doc names
no `__private` item, linked or in code, and says what it does in words: rustdoc
renders a link into a hidden module as its bare path, dead and without a
warning, so `just doc` holds the link (its public pass renders alone, and no
`href` of it may name a `__private`), review the name in code, the hygiene
build an emitted path, and rustdoc's `-D warnings` a doc link that no longer
resolves.

**An expansion builds no tier-1 struct by literal**: it lands in the
developer's crate, where a `#[non_exhaustive]` one is refused. A registry
entry it submits that a program reads has private fields, read through
accessors, and a `const fn` constructor in `__private` (`ProcessMethod`'s
shape; many facts travel as a tier-2 `*Parts` struct); one only the framework
reads stays whole in `__private`, built by literal; a seam a third party
fills by hand has a public constructor (`SocialProviderEntry::new`).

"The use site owns that crate by definition" is not an admissible reason;
owning a capability means enabling its feature.

One exception, not a licence: **emitted derives** (`serde`, `validator`,
`schemars`) target the call site's prelude by construction. The fix is the
derive's `crate = ` override through a surface re-export; until a derive has
one, its path is legal only where the developer's own source writes that
derive. sea-orm's own derives emit relative `sea_orm::` paths, which an entity
module satisfies with `use nest_rs::seaorm::sea_orm;`.

Held by the compile witness `nest-rs-macro-hygiene` (one dependency, one
feature per capability — `manifests-ci.md`, *Shipping a capability*), which holds a
real entity, so `#[expose]`, `#[crud]` and `#[authorize(Action, Entity)]` are
witnessed there like every other decorator, and a `#[subject]` gated on every
edge beside them. The CLI's scaffold tests witness
the generated tree, and **a generated tree witnesses only what it does not also
supply by accident**: the test strips everything else that would pull a crate
in, so what compiles rests on the decorator alone.

## Pairs — one decorator, one item shape

An edge is two decorators (`CLAUDE.md`, hard "no"). The table is closed:

| Edge | on the struct | on the impl |
|---|---|---|
| HTTP | `#[controller(path)]` | `#[routes]` (or `#[crud]`, which re-emits under it) |
| WS | `#[gateway(path)]` | `#[messages]` |
| GraphQL | `#[resolver]` | `#[operations]` (or `#[crud]`) |
| MCP | `#[mcp]` | `#[tools]` |
| queue / schedule / events / health / lifecycle | `#[injectable]` | `#[processor]` / `#[scheduled]` / `#[listeners]` / `#[indicators]` / `#[hooks]` |

The struct half is named for the host role, the impl half for what it collects.
**The half emitting a host's `Discoverable` brings what its edge runs on**
(`nest_rs_codegen::__private::host_runtime_entry`, over its crate's
`__private::RUNTIME`), and a provider-hosted pair, whose `Discoverable` is
`#[injectable]`'s, per dispatched method under its `#[cfg]`s.
One attribute on two item shapes would give two grammars one rustdoc page and
one go-to-definition. Held by `nest_rs_codegen::__private::pair::ALL` — a
`DecoratorPair` is constructible only there, and both halves parse through it —
and a trybuild snapshot per wrong shape.

**MCP's struct decorator is the protocol's name, not its role word.** The role
word went to the impl half, where a host's methods are; `#[mcp]` cannot be
misread as the `#[tool]` the same file carries. Neither name may cover both
shapes.

`#[tools]` absorbs rmcp's router and handler attributes and `get_info` into
generated code. The impls rmcp needs are emitted inside a private child module
that carries the `rmcp` import itself, so the developer's file names no `rmcp`;
the router is generated `pub(crate)` so the parent reads the real tool list, not
an empty fallback. rmcp has no resource router, so `#[tools]` writes
`list_resources`, `list_resource_templates` and `read_resource` itself: a
`#[resource(template = "…")]` reads the URIs its RFC 6570 level-1 template
writes, each variable a typed parameter, and `#[resources]` is the one listing
(`.claude/decisions/mcp-resources-declared.md`). A host's advertised
capabilities are derived from the roles present (`#[tool]` ⇒ `tools`,
`#[prompt]` ⇒ `prompts`, `#[resource]` or `#[resources]` ⇒ `resources`), never
restated. A host that owns its `ServerHandler` — completion, logging levels,
subscriptions, the `tasks/*` trio — writes rmcp directly and has no `#[tools]`
block; `#[tools]` on a trait impl is a compile error saying so. Such a host
declares no operation for its layers to reach, so a layer or a `#[meta]` beside
its `#[mcp]` is a compile error at the attribute: the struct half asks the host
for `ToolsHost`, which only `#[tools]` implements. Completion is that shape's
witness, in `nest-rs-mcp`'s registry suite; no `demo` feature serves one.

## The impl half — one method grammar at every member

A developer moves between impl-half decorators inside one feature, so all of
them read a method the same way, each fact worded once in `nest_rs_codegen`
with a trybuild snapshot per decorator:

- **One role per method.** A second role attribute is a compile error on the
  repeated attribute, never the first taken and the rest ignored.
- **The receiver is a shared borrow.** A host is one instance shared by every
  call: `&mut self` and `self` are refused, and so is a method with no receiver.
  The provider-hosted decorators also accept `self: &Arc<Self>`; the four edges
  take `&self` alone, because WS, MCP and GraphQL call on `&Self` and have no
  `Arc` to lend. The refusal names which borrow each member takes.
- **`fn` and `async fn` are both accepted**; only an `async fn` is awaited.
  Whether a body blocks is the developer's call.
- **A dispatched method is concrete** — no type or const parameter, since
  nothing supplies one; lifetimes are allowed.
- **`#[cfg]` travels with the method**, including a `cfg` inside `cfg_attr`: a
  method compiled out is compiled out of its wrapper, its registration and its
  inventory entry.

**What a method answers is read by its type, never by its spelling.** A
decorator resolves no name, so a renamed `Result` or a type alias must behave
as the literal does. Behaviour is decided through a type probe —
`nest_rs_core::__private::Answer` at `#[routes]` and `#[operations]`,
`nest_rs_mcp::__private::OperationAnswer` at `#[tools]`,
`nest_rs_ws::__private::ReplyValue` at `#[messages]`, `nest_rs_events::__private::ListenerAnswer` at `#[listeners]`.
What a parameter resolves is read the same way: an extractor that resolves a
provider implements `nest_rs_core::ResolvesProvider`, and the impl half asks
`nest_rs_core::__private::ParamProbe` per parameter type — called on
`&&ParamProbe<T>`, the autoref depth its unit tests pin — and hands what it
answers to `Discoverable::injected_per_request`, so the access graph checks it;
so is the `T` a route's `Ambient<T>` reads, an alias included, and an in-band
half asks every parameter it hands its protocol whether it is an `Ambient` or a
`Scoped` (`ParamProbe::decoded`), refusing either behind an alias with the fix.
What a dependency's type tells a refusal is read the same way: each `#[inject]`
field, each such parameter and each service a GraphQL field resolver takes by
reference asks `nest_rs_core::__private::RemedyProbe`, on
`&&RemedyProbe<T>`, whether its type carries a remedy (`CarriesRemedy`;
`Scoped<T>` carries `T`'s), and hands it to `Discoverable::injected_remedies` —
an edge's struct half through its `__nestrs_injected_remedies()`, its impl half
adding its parameters' — which the boot reads only where it refuses one.
Spelling decides only what no value can tell a macro, and each such reading is
stated in the decorator's rustdoc: the wrapper async-graphql's derive itself
reads by name, the `Json<T>` a tool's output schema is read off (`#[tools]` hands
it to rmcp's `output_schema`, the wrapper writing the answer inside its site), the
template variable a `#[resource]` parameter's name
spells, the `Authorized<A, E>` parameter `bind` hands its subject and whose `E`
names the entity, which `#[subscribe_message]` parameter is the payload (`&WsClient` by its reference, `Scoped<T>` and
`Ambient<T>` by their names — each resolved through a function of the edge crate's
`__private` returning core's type, so a namesake does not compile), the
`Scoped<T>` and `Ambient<T>` parameters `#[operations]` and `#[tools]` leave out of
the protocol's arguments, read the same way, a GraphQL field's
`&DataLoader<L>`, whose `L` the boot reads the owner of, and the request parameters an
OpenAPI document lists off the extractor spelling them (`Json<T>`, `Form<T>`,
`Query<T>`, `Header<T>`, `Path<…>`, `Multipart`; an extractor that is the body
says so by type, `nest_rs_http::__private::DocumentedBody`). What a route
answers is no such reading: `#[routes]` documents its status, its headers and
its body through `nest_rs_http::__private::{AnswerProbe, MediaProbe,
PayloadProbe}`, an alias included, and `#[api(response = T)]` names only the body of a handler
building its own `Response`, whose type names none.

## Keys — one grammar, one refusal sentence

Every key a decorator reads is parsed through `nest_rs_codegen::Grammar`; `syn`'s
three meta parsers are `disallowed-methods` (`clippy.toml`), and a site calling one
carries its `#[expect]`, on a foreign attribute alone: its keys are its owner's
(`#[sea_orm]`'s are sea-orm's). A decorator that reports several refusals of
an item at once collects them in `nest_rs_codegen::Refusals`.
An argument a decorator does not accept is a compile error naming the decorator
and what it accepts — one shared sentence; a specific reason is added when it
fits on one line (`CLAUDE.md`, *Families*). A value refusal opens with its site
(the decorator and the key or positional), because that is what a problems list
shows without the source frame. No value is handed to `syn`'s own sentence or to
`format_ident!`, which names neither the decorator nor the key, or panics.

**A misuse rustc reports leads with one framework sentence, naming no expansion
item.** A trait an expansion bounds on carries `#[diagnostic::on_unimplemented]`
whose message and note say the whole fix; a `__private` impl rustc would list as a
candidate, or a blanket that leads it to the wrong trait, is
`#[diagnostic::do_not_recommend]`, unless the candidates are the fix (`ForRootValue`'s
impls, the values a `for_root` takes); and a bound one use site checks is not
re-checked by every probe — what a decorator generates carries no probe its
type cannot answer. rustc's own lines stay around that sentence, since no
`#[diagnostic]` attribute hides `required by a bound in` or `the trait … is not
implemented`, and a bound an expansion holds at several sites repeats it, one error
each (`crud_op_the_service_does_not_offer.stderr`). Held by the hygiene snapshots,
read when they move. One
exception, by the first value: an impl only coherence can refuse (a `Model`'s
wire traits, `.claude/decisions/mask-provenance.md`) reports rustc's own `E0119`,
whose note names the bound, so that bound's name says the reason.

**Every layer attribute is one cell of `nest_rs_codegen::__private::layers`'
table** — an exhaustive `match` over the pairs, each layer attribute and the
scopes (the host, a method, a gateway's connection hook): built, pending or
refused with the fact that makes it meaningless (`a hook has no caller and no
site`), read by `take_layers`, with a trybuild snapshot per refused cell and a
hygiene use site per built one. An attribute a site does not take never runs
as a silent no-op, nor reaches rustc as `cannot find attribute`. **A site's
items are written once**, by `nest_rs_codegen::__private::site_items`, in its
host's inherent `impl` — its layers, its guard stage where its edge runs one,
its `&'static Site` — and read back through `site_ref`. A
provider-hosted pair (`#[processor]`, `#[scheduled]`, `#[listeners]`)
declares each method's site through `site_const`
and reports what it binds to the access graph through `host_layers_entry`;
`#[routes]` declares one per route and `#[messages]` one per message;
`#[operations]` declares one per field beside its handler, its guard stage
included, and submits them for the boot to compose; `#[tools]` declares one
per operation alike and lists them for the boot to compose — so it refuses a
generic impl, and `#[mcp]` a generic struct, whose `static` sites no parameter
reaches.

**`#[meta(EXPR)]` is read alike at every pair**: on the host (the struct, or
the impl block a provider pair decorates) and on each dispatched method, into
the site's one `Handler`, built once; two values of one type at one level are
refused — at expansion when each expression names its type
(`nest_rs_codegen::__private::take_meta`), else when the site composes, by the
type they resolve to (`DuplicateMetaError`).

**The worker-job family answers every key at every member.** `#[process]`,
`#[every]`, `#[cron]` and `#[after]` are one job table in `nest-rs-codegen` (held
by its exhaustive `match`): each cell is built, or refused with the fact that
makes it meaningless (``#[every] takes no `retries`: a tick's retry is the next
occurrence``), with a trybuild snapshot per refused cell and a hygiene use site
per built one. `transactional` and `timeout` are the keys every member builds.
`#[on_event]` is no member — a listener is no worker job — and reads two of the
family's keys, `transactional` and `timeout`, through a sibling grammar
(`nest_rs_codegen::__private::ON_EVENT_GRAMMAR`) with the family's parsers,
named by site.

**A rule both a decorator and the runtime check is written twice and pinned
once.** A surface crate cannot depend on its own macros, so a queue name or a
job key checked on a literal at compile time and on a value at runtime is two
functions. That is the one allowed duplication, and it owes: one test in the
surface crate running both over one corpus, its bounds read from the runtime's
constants; one fact in both sentences — the compile error is the runtime's
sentence with the site in front.

**A provider-hosted decorator states its residency.** `Container::get::<Host>()`
answers only for a singleton under its own type, so every decorator that builds
a provider writes `ProviderResidency` (held by the trait: a contradiction is
`E0119`), its hidden `RESIDENCY` naming the shape — edge host, request-scoped,
transient — so the refusal of an impl half or of `as dyn Trait` says the one
cause it has, naming its decorator and its host, never every cause at once. A
refusal lands at the earliest site that can know it — the host's own
decorator, then the impl half's expansion, then the boot (*Discovery* in
`container.md`) — with a trybuild snapshot per refused shape and one that tries
the escape.

**`#[tool]`, `#[prompt]` and `#[resource]` fall back to the doc comment**
when no `description` is given, so a consumer who writes comments never states
the sentence twice. The attribute wins; an operation with neither, or a blank one,
is a compile error; no other decorator falls back — `#[api]` takes its argument
or nothing. `demo/` and the templates always write the argument (`demo.md`).

**`#[instance]` is a decorator before its third use** because a hand-written
impl would choose what must be read: a marker's name is a namespace segment,
so it is its ident in snake case — read like a `#[config]`'s namespace, refused
at expansion where a namespace could not carry it (`Unnamed`, a `__`, a
reserved word) — and the `Instance` impl's associated type is the decorator's
to write; `Instance` is sealed behind the `__private` supertrait the
decorator implements, so no hand-written impl reaches the boot. Its reserved
words are `nest_rs_codegen`'s list, held to `architecture.md`'s by a test in
`nest-rs-cli`. Its use site in `demo/` is `audio`'s `Transcodes`, whose records keep a Valkey
instance of their own beside the queue's; its runnable rustdoc example and
`nest-rs-config`'s integration suite exercise the rest.

## When (not) to write a decorator

Write one when the pattern appears in three places, the boilerplate is
mechanical, and the rule is teachable in one sentence. Never for business logic,
one-off integrations, inference Rust cannot give (prefer a builder), or anything
needing `unsafe` or runtime reflection.

It ships with rustdoc whose example, on its surface crate's re-export, runs
and proves what the expansion provides, a test in the home crate's suite
(or `nest-rs-testing` for cross-crate wiring), a use site in `demo/`, and its
`nest-rs-macro-hygiene` witness; `#[listeners]` has no use site in `demo/`, for
the reason `edges.md` argues. **Compile cost above 0.5 s per use site is a
defect — measure it.**
