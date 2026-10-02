---
paths:
  - "crates/*-macros/**"
  - "crates/nest-rs-codegen/**"
---

# Decorators — macros and their shared grammar

Decorators are the framework's leverage (`CLAUDE.md`, *Thesis*). The pair rule,
the umbrella's front door and the expansion witness are stated there; this file
is how a decorator is written so that they hold.

## Where a decorator lives

A `proc-macro` crate can export only macros, so each decorator lives in a
companion `*-macros` crate that its surface crate re-exports. Its `lib.rs` holds
the `#[proc_macro_attribute]` items — the one licensed exception to "`lib.rs`
carries no logic", since Rust forces them to the crate root — as thin
delegations into the crate's own modules. What several macro crates share lives
in `nest-rs-codegen`. A `*-macros` crate never depends on its surface crate.

## Every emitted path is rooted

An expansion lands in the developer's crate, so it emits only `::std` / `::core`
paths and paths rooted at `::nest_rs::<concern>::`. A `*-macros` crate whose
expansion also lands in framework crates that do not carry the umbrella reaches
its **own** surface crate's re-export (`::nest_rs_queue::TARGET` from
`nest-rs-queue-macros`), never a sibling's root and never a bare third-party
path — `::anyhow`, `::tracing`, `::uuid` resolve against the consumer's extern
prelude and break every app that lacks that direct dependency. Routing through
the umbrella is also what dissolves the cycles: `nest-rs-guards`,
`nest-rs-authz` and `nest-rs-seaorm` sit above the transports, and only
`::nest_rs::` sits above all of them. Seams that are not public API stay
`#[doc(hidden)]` where they live.

"The use site owns that crate by definition" is not an admissible reason;
owning a capability means enabling its feature.

Two exceptions, neither a licence:

- **Emitted derives** (`serde`, `validator`, `schemars`) target the call site's
  prelude by construction. The fix is the derive's `crate = ` override through a
  surface re-export; until a derive has one, its path is legal only where the
  developer's own source writes that derive. sea-orm's own derives emit
  relative `sea_orm::` paths, which an entity module satisfies with
  `use nest_rs::seaorm::sea_orm;`.
- **poem's `#[handler]`**, which `#[routes]` and `#[crud]` wrap, targets the call
  site's `poem`. That is a known defect, stated on the `/http/` page, not a
  design.

Held by the compile witness `nest-rs-macro-hygiene` (one dependency, one
feature per capability — `CLAUDE.md`, *Shipping a capability*), which holds a
real entity, so `#[expose]`, `#[crud]` and `#[authorize(Action, Entity)]` are
witnessed there like every other decorator. The CLI's scaffold e2e witnesses
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
Every pair is a `DecoratorPair` in `nest_rs_codegen::pair::ALL`, constructible
only there, and both halves parse through it, so the wrong-shape error names the
sibling and is worded in one place. Held by the type, a codegen unit test over
`ALL`, and a trybuild snapshot per wrong shape.

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
`nest_rs_core::Answer` at `#[routes]` and `#[operations]`,
`nest_rs_mcp::OperationAnswer` at `#[tools]`, `ReplyValue` at `#[messages]`.
Spelling decides only what no value can tell a macro, and each such reading is
stated in the decorator's rustdoc: the wrapper async-graphql's derive itself
reads by name, the shape a masked MCP or WS operation unwraps, and what an
OpenAPI document infers (`#[api(response = T)]` states what spelling hides).

## Keys — one grammar, one refusal sentence

Every key a decorator reads is parsed through `nest_rs_codegen::Grammar`; `syn`'s
meta parsers are `disallowed-methods` outside `nest-rs-codegen` (`clippy.toml`).
An argument a decorator does not accept is a compile error naming the decorator
and what it accepts — one shared sentence; a specific reason is added when it
fits on one line (`CLAUDE.md`, *Families*). A value refusal opens with its site
(the decorator and the key or positional), because that is what a problems list
shows without the source frame. No value is handed to `syn`'s own sentence or to
`format_ident!`, which names neither the decorator nor the key, or panics.

**The worker-job family answers every key at every member.** `#[process]`,
`#[every]`, `#[cron]` and `#[after]` are declared once as
`nest_rs_codegen::job`: members × keys in a `match` with no wildcard arm, so a
key or member added without a cell at every crossing does not compile. Each cell is built, or refused with the fact that makes it meaningless
(``#[every] takes no `retries`: a tick's retry is the next occurrence``).
`transactional` is the one key every member builds. Held by the type, a
trybuild snapshot per refused cell, and a hygiene use site per built one.

**A rule both a decorator and the runtime check is written twice and pinned
once.** A surface crate cannot depend on its own macros, so a queue name or a
job key checked on a literal at compile time and on a value at runtime is two
functions. That is the one allowed duplication, and it owes: one test in the
surface crate running both over one corpus, its bounds read from the runtime's
constants; one fact in both sentences — the compile error is the runtime's
sentence with the site in front.

**A provider-hosted decorator states its residency.** `Container::get::<Host>()`
answers only for a singleton under its own type, so every decorator that builds
a provider writes `ProviderResidency`, `true` or `false`, and a contradiction is
`E0119` — a fact read from a missing marker could be filled by hand. A refusal
lands at the earliest site that can know it: the host's own decorator, then the
impl half's expansion, then the boot (*Discovery* in `container.md`). Held by a
trybuild snapshot per refused shape plus one that tries the escape.

## When (not) to write a decorator

Write one when the pattern appears in three places, the boilerplate is
mechanical, and the rule is teachable in one sentence. Never for business logic,
one-off integrations, inference Rust cannot give (prefer a builder), or anything
needing `unsafe` or runtime reflection.

It ships with rustdoc showing the expansion, a test in the home crate's suite
(or `nest-rs-testing` for cross-crate wiring), a use site in `demo/`, and its
`nest-rs-macro-hygiene` witness. **Compile cost above 0.5 s per use site is a
defect — measure it.**
