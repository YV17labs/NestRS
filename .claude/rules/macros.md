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

An expansion lands in the developer's crate, so it emits only `::std` / `::core`
paths and paths rooted at `::nest_rs::<concern>::`. A `*-macros` crate whose
expansion also lands in framework crates that do not carry the umbrella reaches
its **own** surface crate's re-export (`::nest_rs_queue::TARGET` from
`nest-rs-queue-macros`), never a sibling's root and never a bare third-party
path — `::anyhow`, `::tracing`, `::uuid` resolve against the consumer's extern
prelude and break every app that lacks that direct dependency. Routing through
the umbrella is also what dissolves the cycles: `nest-rs-guards` sits below
the transports, `nest-rs-authz` and `nest-rs-seaorm` above them, and only
`::nest_rs::` sits above all of them. One place roots them:
`nest_rs_codegen::reroot`, which every decorator calls on its finished
expansion, and which follows a path an attribute pulled in (`#[authorize]`,
`#[crud]`) with the umbrella's check for its feature, under the `cfg`s around
that path.

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
renders a link into a hidden module dead without a warning, so review holds
that, the hygiene build an emitted path, and rustdoc's `-D warnings` a doc link
that no longer resolves.

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
witnessed there like every other decorator. The CLI's scaffold tests witness
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
an empty fallback. A host's advertised capabilities are derived from the roles
present (`#[tool]` ⇒ `tools`, `#[prompt]` ⇒ `prompts`), never restated. A host
that owns its `ServerHandler` (resources, completion) writes rmcp directly and
has no `#[tools]` block; `#[tools]` on a trait impl is a compile error saying so.
`demo`'s `posts` host stays on that raw shape as its witness.

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
`nest_rs_mcp::__private::OperationAnswer` at `#[tools]`, `ReplyValue` at
`#[messages]`, `nest_rs_events::__private::ListenerAnswer` at `#[listeners]`.
What a parameter resolves is read the same way: an extractor that resolves a
provider implements `nest_rs_core::ResolvesProvider`, and the impl half asks
`nest_rs_core::__private::ParamProbe` per parameter type — called on
`&&ParamProbe<T>`, the autoref depth its unit tests pin — and hands what it
answers to `Discoverable::injected_per_request`, so the access graph checks it.
Spelling decides only what no value can tell a macro, and each such reading is
stated in the decorator's rustdoc: the wrapper async-graphql's derive itself
reads by name, the shape a masked MCP or WS operation unwraps, and what an
OpenAPI document infers (`#[api(response = T)]` states what spelling hides).

## Keys — one grammar, one refusal sentence

Every key a decorator reads is parsed through `nest_rs_codegen::Grammar`; `syn`'s
three meta parsers are `disallowed-methods` (`clippy.toml`), and the one site
calling one carries its `#[expect]`: a foreign attribute's keys are its owner's
(`#[sea_orm]`'s are sea-orm's). A decorator that reports several refusals of
an item at once collects them in `nest_rs_codegen::Refusals`.
An argument a decorator does not accept is a compile error naming the decorator
and what it accepts — one shared sentence; a specific reason is added when it
fits on one line (`CLAUDE.md`, *Families*). A value refusal opens with its site
(the decorator and the key or positional), because that is what a problems list
shows without the source frame. No value is handed to `syn`'s own sentence or to
`format_ident!`, which names neither the decorator nor the key, or panics.

**Every layer attribute is one cell of `nest_rs_codegen::__private::layers`'
table** — an exhaustive `match` over the pairs, each layer attribute and the two scopes
(the host, a method): built, pending or refused with the fact that makes it
meaningless (`a hook has no caller and no site`), read by `take_layers`, with a
trybuild snapshot per refused cell and a hygiene use site per built one. An
attribute a site does not take never runs as a silent no-op, nor reaches rustc
as `cannot find attribute` — but at the WebSocket `#[on_connect]` /
`#[on_disconnect]` hooks, no row yet, until the hooks move onto it. A
provider-hosted pair (`#[processor]`, `#[scheduled]`, `#[listeners]`)
declares each method's site through `nest_rs_codegen::__private::site_const`
and reports what it binds to the access graph through `host_layers_entry`.

**`#[meta(EXPR)]` is read alike at every pair**: on the host (the struct, or
the impl block a provider pair decorates) and on each dispatched method, into
the site's one `Handler`, built on first read; two values naming one type at one
level are refused (`nest_rs_codegen::__private::take_meta`).

**The worker-job family answers every key at every member.** `#[process]`,
`#[every]`, `#[cron]` and `#[after]` are one job table in `nest-rs-codegen` (held
by its exhaustive `match`): each cell is built, or refused with the fact that
makes it meaningless (``#[every] takes no `retries`: a tick's retry is the next
occurrence``), with a trybuild snapshot per refused cell and a hygiene use site
per built one. `transactional` is the one key every member builds.
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
`E0119`). A refusal lands at the earliest site that can know it — the host's
own decorator, then the impl half's expansion, then the boot (*Discovery* in
`container.md`) — with a trybuild snapshot per refused shape and one that tries
the escape.

**`#[tool]` and `#[prompt]` fall back to the doc comment** when no
`description` is given, so a consumer who writes comments never states the
sentence twice. The attribute wins; an operation with neither, or a blank one,
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
`nest-rs-cli`. It has no use site in `demo/`, where no app opens a resource
twice: its runnable rustdoc example and `nest-rs-config`'s integration suite
exercise it until a demo app needs a second instance, and the use site lands
with that need.

## When (not) to write a decorator

Write one when the pattern appears in three places, the boilerplate is
mechanical, and the rule is teachable in one sentence. Never for business logic,
one-off integrations, inference Rust cannot give (prefer a builder), or anything
needing `unsafe` or runtime reflection.

It ships with rustdoc whose example, on its surface crate's re-export, runs
and proves what the expansion provides, a test in the home crate's suite
(or `nest-rs-testing` for cross-crate wiring), a use site in `demo/`, and its
`nest-rs-macro-hygiene` witness. **Compile cost above 0.5 s per use site is a
defect — measure it.**
