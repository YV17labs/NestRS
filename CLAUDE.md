# CLAUDE.md — nestrs

Durable decisions. The code says what *is*; this file says what was
**decided** and must be **respected**. Not a code map — layout,
signatures and versions live in the code.

Public repo. No machine-local paths, no private references.

Zone-specific rules load on demand from `.claude/rules/` when you touch
a matching file. This file is the always-loaded core: thesis,
invariants, naming, and what "done" means.

## Thesis

nestrs is an opinionated Rust framework whose thesis is **the developer
writes business logic; the framework carries the rest**. Cross-cutting,
error-prone concerns — **authn, authz, row-level filtering,
transactions, edge validation, discovery, lifecycle** — must be
*transparent*. Forcing the developer to hand-manage any of them is a
framework defect.

The leverage is **procedural macros** — decorators, as declarative in
Rust as in TS. Reach for one first.

A framework has **no local change**. Every name, default, error sentence
and declaration is met by every feature that will ever use it, so the
unit of design is the whole surface, never the corner that motivated the
work — see *The ask names a site; the design answers the family*.

## Naming is the pillar — read this before anything else

**A name and its path say the same thing.** From a path you know the type; from
a type you know where the file is. Every other rule in this file assumes that
property, because a reader who cannot navigate cannot check anything else — a
misplaced concern, a missing guard, a silent failure are all invisible to
someone who cannot find the file. **This is the single most important rule in
the project.**

It is also the rule most reliably under-weighted, and the failure mode is
specific: a name reads fine *in isolation* and is only wrong *against its
location*. `ThrottlerModule` is a perfectly good name; `ThrottlerModule` inside
`nest-rs-redis` is a defect, because nothing in it says which backend you are
looking at when it appears in a stack trace at three in the morning. So a name
is **never judged alone** — always as the qualified path a caller types, and
always against its siblings at the same level.

Three consequences, and they are not negotiable:

1. **The framework's crate name is a subject, and a driver carries it.** The
   bare name of a capability belongs to the crate that defines the port. A crate
   that *implements* somebody else's port prefixes every module it declares with
   its own subject — `nest_rs::throttler::ThrottlerModule` is the port's own
   binding, `nest_rs::redis::RedisThrottlerModule` is Redis's. This mirrors what
   the implementations already do (`InMemoryThrottler`, `RedisThrottler`), and
   the stutter at the path is an accepted cost: a name that is unambiguous in a
   log outranks a name that is short in an import.
2. **The stem is the path.** A module's type name is the crate subject plus
   every folder below `src/`, joined — `redis/queue/module.rs` is
   `RedisQueueModule`, `audio/http/module.rs` is `AudioHttpModule`. Adapters
   read the same way: `posts/http/controller.rs` is `PostsController`.
3. **Siblings follow one scheme.** One odd member means either it or the scheme
   is wrong, and deciding which is the finding — never a shrug. A type renamed
   without its `*Setup`, its `*Host` or its config is half a rename.
4. **A variable is a path too.** A `#[config]`'s namespace is the same stem its
   type is named from — crate subject, then binding folders, joined by `__` — so
   `SeaOrmConfig` reads `NESTRS_SEAORM__URL` and `RedisWorkerConfig` reads
   `NESTRS_REDIS__WORKER__*`. From a variable a reader finds the type; from a
   module they find the variable. A resource word that names neither
   (`NESTRS_DATABASE__URL`) is the defect, however universal the convention.

The composition root follows from the same law and has **three module shapes,
no fourth** — `<Vendor>Module::for_root` opens a resource, `<Port>Module::for_root`
carries a capability's policy, `<Vendor><Port>Module` binds one to the other —
and the framework is *Ports & Adapters*: the port owns the semantics, the
adapter owns only the transport. The model is in `.claude/rules/architecture.md`.

The full model, the tables and the one documented precedence live in
`.claude/rules/architecture.md`; `naming.rs` in `nest-rs-conformance` enforces
what is mechanical, with an empty baseline that only shrinks. **When a naming
question and any other question compete for the answer, naming wins** — settle
it first, then build.

## Rule priority — Rust first, conventions second

Both, in order. When they conflict, **Rust wins** — adapt the
convention, don't bend Rust.

1. **Rust (non-negotiable).** Idiomatic, reviewable: orphan/coherence,
   explicit errors (`thiserror` in libs — no silent failure, no
   swallowed `DbErr`), **zero `unwrap`/`expect` on framework hot paths**
   (tests and one-shot bootstraps may use them), honest APIs
   (`Type::new(deps)` when tests need it), `Result` propagated to the
   transport boundary. Macro-emitted `impl` blocks don't excuse hiding
   errors or bypassing `Repo`.
2. **Conventions (second).** Module/feature folders, decorator names,
   thin handlers, one `service.rs` per feature. Conventions = *where*;
   Rust = *how*.

## Hard "no" list

Violating one is never a shortcut — it is a defect. If a task appears
to require it, **stop and ask**.

- **No authn/authz decision outside a guard.** Only `#[use_guards]` + a
  visible `#[authorize]`/`#[public]` declare posture. A parameter type
  (`Authorized<A, E>`), a service method, or a binding helper is never
  the check. Every check must be greppable as one of those three sites.
- **No data access outside a service; no service reaching the DB outside
  `Repo`.** The named exceptions are listed in the data-layer rule;
  there are no others.
- **No silent failure.** Never return `[]`/`None` when the DB errored —
  batch and loader methods return `Result`. Never log-and-pretend-success.
- **No external DI library.** Ours is internal by decision. Extend it.
- **Four NestJS surfaces are refused by design**, not deferred. They were
  recorded in a roadmap that no longer exists, and each is a defect if it
  reappears:
  - **No microservice transport split.** `@nestjs/microservices` is a
    second dispatch model beside HTTP. Here an app is one binary serving
    the edges it imports; two binaries share `demo/crates/features` and
    the database and **never RPC each other**.
  - **No `ClassSerializerInterceptor`.** Exposure is `#[expose]` on the
    entity and the mask is the caller's ability. A serializer-shaped
    third place a column can be shown or hidden from is what the
    fail-secure design exists to not have.
  - **No `HttpModule` / `HttpService`.** A general-purpose *outbound*
    client is not a framework concern: an app writes `reqwest` and
    injects its own. Wrapping it costs the caller every option the client
    has and buys a module. (A crate that needs one for its own protocol —
    `nest-rs-authn`'s OAuth exchange, storage's presigned PUT — holds it
    privately; that is not a surface.)
  - **No bundled `Logger`.** `tracing` is the ecosystem's, and the span
    targets under *Observability* are the contract. A framework logger
    would be a second one, and the first thing to drift from it.
- **No renaming the umbrella crate.** The facade stays `nest-rs`, every
  sub-crate `nest-rs-*` (paths `nest_rs_*`, span targets
  `nest_rs::<concern>`). The `nestrs` brand (CLI, `NESTRS_*` env,
  nestrs.dev) deliberately differs — accepted, not a bug to fix.
- **No env-var name spelled as a literal.** `NESTRS` is the *deployment's*
  default prefix, not a fixture: `NESTRS_ENV_PREFIX=ACME` on the process
  renames every framework variable at once. So a name is always built —
  `nest_rs_config::var_name(ns, key)` or `EnvPrefix::var(name)`, never
  `"NESTRS_AUTHN__SECRET"` in a message, a check or a template. Three
  exceptions, all because they are not the app's: `RUST_LOG` (the
  ecosystem's), `NESTRS_NO_BOOTSTRAP` (the CLI tool's own), and
  `NESTRS_ENV_PREFIX` — the bootstrap's, and the one name no prefix can
  rename. That last one is spelled once per crate that needs it
  (`EnvPrefix::VAR`, `context::ENV_PREFIX_VAR`) and referenced from there.
  The prefix is set **on the process** — container, shell, Justfile —
  never in `.env`, which is read after it has already chosen the cascade.
- **No collapsing the two workspaces.** `demo/apps/` and
  `demo/crates/features/` are fixed names.
- **No feature flags for capabilities that don't exist yet.**
- **No decorator that forces a manifest line.** A macro expansion never
  obliges the developer to declare a crate. See *The umbrella is the
  front door*.
- **No backwards-compatibility shims** — no public API to preserve yet.
- **No mocking the database in e2e tests.**
- **No flat `tests/<x>.rs` and no third suite name.**
- **No umbrella module re-exporting every edge of a feature.**
- **No transport-level discovery without module-gating.** *Discovery* is
  anything an `inventory` submission would **mount or expose** — a route, a
  resolver, a tool, a loader — and the gate is what stops a linked crate from
  serving through an app that never imported it. A **request-scoped forwarder**
  is not that: it mounts nothing, exposes nothing, and copies a value only if
  something already put that value on the request. Its gate is therefore the
  module-gated guard that *produces* the value, and a second gate below it buys
  an empty marker struct in every consumer — the defect *The product's own*
  names. So `forward_principal!` takes the principal type alone. Recorded
  because the argument reads as a relaxation and is not one: the condition that
  activates the forwarder is strictly narrower than a provider registration,
  since a registered marker fires the forwarder whether or not anyone
  authenticated, and the value's presence does not.
- **No two decorators for the same concern** — deprecate first.
- **No capability built for the site that asked.** A need surfaces at one
  member of a family and is answered for the family: every other member is
  built, refused in a sentence, or raised as an owner question. Fitting the
  answer to the caller that raised it is the defect, whatever it costs to
  widen. See *The ask names a site; the design answers the family*.
- **No declaration designed for one site.** Anything the framework
  interprets is designed against every site the underlying standard
  permits — one grammar wherever it is possible, a compile error naming
  the standard's limit wherever it is not. Silence at a site is the
  defect, and an ignored argument, a bare "unknown key" and an
  unmentioned gap are all silence. See *One declaration, every site the
  standard permits*.
- **No decorator on two item shapes.** An attribute macro is one path in the
  macro namespace: the shape is discriminated *after* `syn::parse`, so a name
  worn by both a struct and its `impl` gives one rustdoc page for two argument
  grammars, one symbol for go-to-definition, and the same
  `in this expansion of #[x]` note whichever half actually failed — the
  compiler cannot tell the reader which decorator it is looking at, and neither
  can `rg`. So an edge is **two** decorators: the host on the struct, and on the
  impl a sibling **named for what it collects** — `#[controller]`/`#[routes]`,
  `#[gateway]`/`#[messages]`, `#[resolver]`/`#[operations]`, `#[mcp]`/`#[tools]`.
  The wrong shape is a **compile error naming the sibling**, never "expected
  struct". Testable form, both halves checkable: **both halves parse through one
  `DecoratorPair` const** (`rg 'DecoratorPair' crates/*-macros/src/` names every
  pair, and `nest_rs_codegen::pair` is the only place either sentence is worded),
  and **each pair ships a trybuild snapshot per wrong shape**. An impl-half
  decorator whose struct half is the generic `#[injectable]` — `#[processor]`,
  `#[scheduled]`, `#[listeners]`, `#[indicators]`, `#[hooks]` — uses
  `DecoratorPair::on_provider` and owes the same named error.
- **No second way to configure a module.** `Module::for_root(x)` takes one
  value carrying everything the app declares; the `*Setup` it returns is
  opaque. A builder chain on it, a second constructor on the module type,
  or a `#[config]` reachable only through the environment are all the same
  defect. See *`for_root` — one seam, one value, no chain* in
  `.claude/rules/framework.md`.
- **No new third-party crate without a release in ~12 months.** A
  failing candidate must be flagged explicitly, never adopted silently.
- **No third-party version requirement outside `major.minor`.** Two
  components, everywhere, in every manifest the repo owns *and*
  generates: the minor is the floor we build against, the patch is the
  publisher's. `"1"` and `"1.53.1"` are both defects. Move the minor
  with `cargo update`; a semver-*incompatible* release is reported to
  the owner, never taken. One documented exception, at its pin. See
  `.claude/rules/manifests-ci.md`.

## Two workspaces — framework vs. product

- **`crates/nest-rs-*` (root workspace) — the framework.** Generic,
  publishable, product-agnostic. Never names a concrete `Claims`,
  entity or policy — generic *over* them. No runnable app.
- **`demo/` — the product** (the "Publish" demo). Its own workspace
  (`apps/*` + `crates/*`), consuming the framework by **relative path**.
  `cd demo` and drive it as its own repo: `nestrs run`, `.env` cascade,
  `Justfile`, its own `Cargo.lock`/`target/`.

Each builds, tests and locks on its own. A change spanning both compiles
in `demo/` — the path dep pulls live framework source.

**Dividing rule:** `demo/crates/features/` when *any other app could
reuse it*; `demo/apps/<x>/` only when *this app's exposure decides
something the feature can't generalize*.

**`demo/` carries no comments. None — owner's rule, not a preference to
weigh, and it has no exceptions.** No `//`, no `//!`, no `///`. This is
demonstration code: the shorter it is, the better it reads for the
developer copying it, and a comment there has always been something to
delete. The framework explains itself in prose because it is a library
whose *why* is not in the code; the product does not. If a line seems to
need one, the code is wrong — rename it, split it, or move the decision
into `CLAUDE.md` where decisions live. This binds the whole workspace:
`apps/`, `crates/`, tests, `build.rs`.

**It binds `demo/` and stops there** — it is *this* paragraph, and the next one
is a different rule with a wider reach. A scaffolded project is not
demonstration code: it is the developer's own repository, and the framework has
no standing to impose its house aesthetic on it. So a CLI template may carry a
comment, and the `// SECURITY:` block above a generated `#[public]` route is the
case that decides it — that note is the framework teaching at the one line where
the developer will change the posture, and moving it to a file they may never
open makes the scaffold less safe, not shorter. The rule the templates *do* owe
is the one below — **that** paragraph names them, and this one does not. Read
the two as one and you get a rule neither states.

**Prose the framework compiles into behaviour is declared as an argument,
never as a doc comment.** A `#[tool]` / `#[prompt]` description is the
sentence a language model reads to choose the operation, so it cannot
just be dropped — it moves to `#[tool(description = "…")]`, which the
decorator accepts precisely so the prose is a value rather than a
comment. Same for `#[api(summary = …, description = …)]`. In `demo/` that
attribute form is the only form; a doc comment on an operation is the
rule being broken, not the exception to it. **This is the rule for the
code the repo writes and generates** — `demo/`, and every CLI template.

The *framework* is one notch wider, and deliberately: `#[tool]` /
`#[prompt]` fall back to the doc comment when the attribute states no
`description`, so a consumer who does write comments never authors the
sentence twice. The attribute always wins, and an operation with
**neither is a compile error** — a description is not optional. Only
these two decorators have that fallback; `#[api]` takes the argument or
nothing.

## The umbrella is the front door

A developer installs **one** crate — `nest-rs`, with the feature for the
capability — and writes code. Cargo resolves the rest. This is the norm
for Rust frameworks (tokio, bevy, rocket, tauri), and it is what makes
the *thesis* true at install time, not just at call time.

- **A macro never makes the developer declare anything.** Every path an
  expansion needs is rooted at `::nest_rs::<concern>::` — never at a
  sibling crate. Seams that are not public API stay `#[doc(hidden)]`
  where they already live; the root is what matters, not the module.
  A decorator whose expansion forces a second `nest-rs-*` line into the
  consumer's manifest is a framework defect, not a documented caveat.
- **Sub-crates are compilation units, not the install surface.** They
  stay published and `nest-rs-*` named; the docs stop presenting them as
  an entry point. Renaming the `nest-rs` dependency is unsupported —
  the same limit tokio has, for the same reason.
- **"The use site owns that crate by definition" is not a reason to make
  it declare one.** Owning a capability means enabling its feature. That
  argument, once accepted, justifies every line in a stanza.
- **The developer's manifest names only what *their own source* names.**
  `serde`, `anyhow`, `sea-orm` stay when their code writes them; every
  `nest-rs-*` beyond the umbrella goes. Shrinking past that is
  over-reach — it costs them feature control and a readable `cargo tree`.
- **A capability that cannot hold "one dependency" is reported**, never
  absorbed into prose in an `## Install` stanza. Adapting that prose is
  what produced the state this rule replaces.

**Shipping a new capability** means all of this, or it is not shipped:

1. A feature in the umbrella's matrix pulling **everything its decorators
   emit unconditionally** — a satellite left out is an `E0433` inside a
   macro expansion, blamed on the attribute.
2. A `pub use nest_rs_<x> as <x>;` re-export, so `::nest_rs::<x>::` resolves.
3. `cargo add nest-rs --features <x>` in **both** the crate README (the
   crates.io landing page) and the docs page's `## Install`. A README
   saying `cargo add nest-rs-<x>` is an unfinished capability.
4. Any derive the decorator emits routed through the surface crate **with
   its `crate = ` override**, so the use site declares neither the crate
   nor a version to keep aligned.
5. Two witnesses, and they are **inside the framework** — there is no
   snippet crate outside the workspaces, and adding one is out of scope:
   - **The expansion** — a use site in `nest-rs-macro-hygiene`, whose one
     dependency proves a decorator needs no second manifest line. It holds
     *decorators only*: a module import there proves nothing about a
     macro, and squats a proof that belongs next door.
   - **The composition** — a test in the capability's **own crate** that
     boots the documented wiring through `nest_rs_testing::TestApp` (or
     `App::builder`) and asserts what a caller gets back. Every `for_root`
     seam has one; that is the bar. Composition is *executed*, never
     merely compiled — a boot test also proves the access graph, the
     resolved config and the mounted routes, which compiling cannot.

   This mirrors the split every framework of this shape settles on:
   `@nestjs/testing` + `sample/` — a testing module and real example
   apps, not a parallel scratch project. `demo/` is our `sample/`; a page
   whose snippet has no counterpart in `demo/` or in the owning crate's
   suite is undocumented, not under-tooled.

   **Assert against shared constants, never a copied literal.** A test
   that re-types `"posts:read audio:transcode"` passes while the policy
   and the deployment drift apart; one that reads
   `features::authz::constants::ALL` fails the moment they do.

**One exception, and only one: binaries.** `cargo add` puts a *library* in
a manifest; `cargo install` puts a *command* on `PATH`. `nestrs` is a
command, so `cargo install --locked nest-rs-cli` is correct and there is no
`--features cli` that could replace it — the same split every Rust tool has.

The compile-time witness is `nest-rs-macro-hygiene`: one dependency,
`nest-rs`, all features. If its manifest needs a second line, the rule
is broken.

## The ask names a site; the design answers the family

An ask always arrives at one site, because a site is where the developer hit
the gap. **The site is where the need surfaced; it is never the scope of the
answer.** So the first move on any work is to widen before writing: name the
family the site belongs to, derive its members, and settle all of them before
building at one. A **family** is any set the framework holds several
interchangeable members of; the ask never names it, because the caller sees
only the member they use. This is the general posture — *One declaration,
every site the standard permits* is it applied to what the framework
interprets, `.claude/rules/testing.md` to what the suite asserts — and it
binds **behaviour** as much as grammar.

**Widening the design is not widening the work.** What may never be one member
is the *decision*; the deliverable still can be. Each member ends in exactly
one of three states, and there is no fourth:

- **built** — its standard permits the thing there;
- **refused** — a compile error or a written sentence naming the fact that
  makes it impossible there, under the clauses below;
- **raised** — possible but unbuilt, hence an owner question, asked as one and
  never written as a refusal.

A proposal states the family and the state of every member. One that names only
the site the ask came from is incomplete, not concise, and a member left
unstated is the silence these rules exist to forbid.

**A defect widens the same way a capability does.** Found at one member, it is
presumed at every other until each is checked; a fix scoped to the member that
reported it repairs the report, not the defect.

**A concern the thesis calls transparent is family-wide by definition**, so
delivering it at the member that asked leaves it untrue everywhere else —
worse than absent, because the capability now reads as present. Whether the
gap is visible from the member's own code decides nothing: the framework is
what the whole family sees.

**None of this licenses an abstraction.** Family-wide are the decision, the
vocabulary, the defaults and the sentence; a shared implementation still waits
for its second real user (*No premature abstraction*). Designing wide and
building narrow is the intended outcome — building wide for one user is a
different defect, not compliance with this one.

## One declaration, every site the standard permits

You are working on a framework, so a declaration is never local. Anything
the framework itself interprets — every key, attribute, seam and default
whose meaning the framework assigns rather than the developer — is designed
against **every site it can reach**, and it reaches every site the
underlying standard permits. A **site** is any member of a family the
framework holds several interchangeable implementations of; transports are
the loudest such family and never the only one.

Where the standard permits it, it is declared **one way**: same key, same
grammar, one shared parser in `nest_rs_codegen`, so learning it once is
learning it everywhere. Where the standard does not, that key is a
**compile error naming the fact that makes it impossible** — never an
ignored argument, never a bare "unknown key". The refusal is what makes the
unification affordable: what an unsupported site owes is a *sentence*, not
an implementation, so the rule can bind everywhere without stalling on the
site that cannot follow.

**The site that cannot is never the ceiling for the sites that can.** A
standard missing the thing at one site subtracts that site, not the
capability. Build it wherever the standard has it and refuse it at the
rest — never level every site down to the poorest, never emulate the thing
where its standard has none so the table looks square, never drop it
because one site cannot follow. Four of five is four, and the fifth owes a
sentence, not a stub.

Three clauses keep the refusal honest, each load-bearing:

- **Cannot is not the same as not yet.** A refusal asserts a property of
  the standard. A site where the thing is possible but unbuilt is an owner
  question, raised as one; writing its refusal instead ships a false
  statement inside the compiler, where no reader is positioned to
  contradict it.
  Invoking the standard is not naming it — the sentence carries the fact a
  reader can check, or it is not a refusal.
- **Refusals are shared, not per key.** One helper, one sentence, every key
  it covers, one trybuild snapshot per site — `reject_http_only_layers` is
  that shape already built. Per-key refusals multiply with the matrix, and
  what multiplies is what gets skipped.
- **The rule demands that the answer be stated, not that it be identical.**
  An asymmetry argued and recorded in `.claude/rules/` satisfies it;
  silence never does, at any site.

What unifies is the **grammar and the sentence**, not the implementation: a
shared abstraction still waits for its second real user, so nothing here
licenses a generic layer built for one. Testable form — the grammar is
worded once in `nest_rs_codegen` and nowhere else, and every refusing site
ships a trybuild snapshot. A capability shipped at one site is shipped
**half**: the others owe either the declaration or the sentence.

## Naming — strict

**The model lives in `.claude/rules/architecture.md`** — five naming levels,
crate types, the provider decision procedure, the role tables, precedence, and
the reserved vocabulary. That file has no `paths:` header, so it is loaded in
every session; read it there rather than restating it here.

It is **one copy, not a description of one.** The file sits in
`crates/nest-rs-cli/src/templates/` and is symlinked into `.claude/rules/`:
the CLI embeds it with `include_str!` so that every scaffolded project's
`AGENTS.md` carries the same bytes this repo works under. The real file is on
the *build's* side and the symlink on `.claude/`'s — never the reverse, since a
checkout without symlink support would then embed a filename into every
generated project and still compile. **Editing the rules means editing that
file** — a second copy anywhere is the defect it exists to prevent.

Three consequences are load-bearing enough to repeat here:

- **The project name stops at the workspace.** No crate, app-level module or
  provider below the composition root carries it.
- **`module.rs` is the DI module; `mod.rs` is the folder index *and* the
  export contract.** Never merged. **No `*_module.rs` ever.**
- **A file exists only if it has real content**, and errors live in
  `error.rs` — never scattered inside `service.rs`.

## Engineering posture

- **No premature abstraction.** Extract after a pattern appears twice.
- **Strict typing.** Enums over string states. Parse at the edge
  (`validator`, `uuid` v7). Newtypes for *meaning*, not format. Avoid
  `Box<dyn Any>` / `serde_json::Value` passthrough.
- **Errors at boundaries**: `thiserror` in libs, `anyhow` at app entry.
- **Doc comments only when the *why* is non-obvious** — never paraphrase
  the name.
- **Security is primordial**: denials and security events log at
  `warn`+, never `debug`.
- **One way to do a thing.** Deprecate before adding a second.

## Observability

- **Span targets are dotted, lowercase, and rooted at the crate that
  emits them.** One target per concern per crate; the crate picks the
  root, the concern picks the tail, and the table is closed:

  | Emitting crate | Target |
  |---|---|
  | a `nest-rs-*` framework crate | `nest_rs::<concern>` — `nest_rs::http`, `nest_rs::orm` |
  | the shared feature library | `features::<feature>` — `features::users` |
  | an app crate, or a standalone crate | `<app>::<concern>` — `api::users` |

  **A crate in a family roots its target at the family**, so the target and the
  path a caller types are the same string: `nest-rs-oauth-client` emits on
  `nest_rs::oauth::client` and is reached at `nest_rs::oauth::client`. The
  concern is then two segments rather than one, which is the *Family* level of
  `.claude/rules/architecture.md` reaching the target table — and it is the
  reason the family gets a filter of its own: `nest_rs::oauth=off` silences
  three crates because they are three roles of one standard, which is a
  hierarchy `EnvFilter` was built to express. That is prefixing on purpose, and
  it is the one case *no target may prefix another* does not name: the rule
  forbids an **accidental** prefix between unrelated concerns — `access` over
  `access_graph` — never a level a reader can see in the path.

  **The root is the crate, never the product.** A feature living in
  `features` stays `features::…` even in a single-product repo whose
  binary has another name — the target's one job is to say where the
  event was emitted, and a product name over the wrong crate destroys
  that.
- **Level per layer.** Controllers/resolvers/gateways: `info` on success.
  Services: `debug`. `Repo`: `trace`. Denials/security: `warn`+.
  Unexpected errors: `error`.
- **Message + fields, never interpolation.** Output is JSON: a constant
  event-name message (`"mounted route"`, not `"GET /v1/users mounted"`)
  plus dynamic data as **structured fields**. Never bake values into the
  message or hand-format columns.
- **Metadata is mandatory — a bare log is a defect.** Every event carries
  ≥1 structured field. A `warn`+ denial emitted bare is a security gap,
  not a style nit: those are the events queried under incident.
- **One event, said once.** Don't restate what a field or the enclosing
  span carries; don't emit the same event at two layers.

### Correlation — W3C Trace Context, in the kernel, never optional

**The framework's correlation primitive is W3C Trace Context and it lives in
`nest-rs-core`.** A `trace_id` names the whole distributed operation; a `span_id`
names one unit of work inside it — an HTTP request, a WS message, an MCP
operation, a queue job, a scheduled tick — and each names the span that caused
it. Minted by whoever *accepts* the work, or continued from what carried it.

**The standard replaced a homemade `request_id`, and the argument is recorded
because it will be re-litigated.** That id answered the question `trace_id`
answers, so the two were duplicates. Three defences were offered and the
specification retires all three: a forgeable inbound id is answered by the
spec's own *restart trace* mutation at a trust boundary, not by a second
identifier; the standard needs 16 random bytes and a `format!`, not an SDK; and
a trace-id sorts by time if you choose bytes that do — ours are a UUID v7's, so
the value is a conformant trace-id **and** an ordered UUID, and its right-most 7
bytes still satisfy the `random-trace-id` flag. The deciding fact is the
ecosystem's: OpenTelemetry's semantic conventions have **no request-id concept**,
only `http.request.header.x-request-id` — a *captured* header. `X-Request-Id` is
therefore upstream data, recorded as an attribute behind a trusted peer, and
never an identity.

A primitive cannot be optional, so this depends on no crate the app may decline,
no collector, and no authentication. Reaching it is `current_trace_id()` /
`current_span_id()`, the same names at every edge; `current_traceparent()` is
what an application injects into its own outbound client, since the framework
ships none.

**Every edge opens exactly one operation span per unit of work, through
`operation_span!`.** That macro is the only place the canonical field vocabulary
is written — `trace_id`, `span_id`, `parent_span_id`, `actor_id` — and declaring
is the whole mechanism: `tracing` fixes a span's fields at creation, so a field
no span declared can never be recorded and a `record` call at such a site is a
**silent no-op**. That is exactly how `actor_id` came to be recorded nowhere on
any transport while the rules claimed otherwise. **Never declare a field nothing
fills** — the same defect wearing the opposite sign.

**Trust is one decision, one list.** An inbound `traceparent` is continued only
from a peer in `trusted_proxies` — the same evidence `X-Forwarded-For` is weighed
on — and restarted otherwise, which is what the spec defines a front gate to do.
An ungated extraction let a public client choose the trace its request joined and
set `sampled` on traffic it generated, which is the denial-of-service surface the
spec names. `tracestate` is forwarded **verbatim** wherever the trace is
continued (a MUST), and dropped with the `traceparent` it belongs to wherever it
is not.

**Two values ride beside the trace and neither is traceability.** `actor_id` is
written by the authn guard when it resolves a principal — absent means anonymous
— and is an **audit identity, never an authorization input**: branching on it in
a service is the defect *no authn/authz decision outside a guard* forbids, and
what a caller may do is the ambient `Ability`. It is ambient as well as on the
span (`set_actor_id`, write-once; `current_actor_id()` anywhere the framework
carries work), and no sentinel is ever returned — `""` or `"anonymous"` would be
indistinguishable from an actor named that. The **sampling flag** is shared and
updatable for the mirror reason: an installed sampler decides after the span
exists, and an outbound `traceparent` carrying a decision nobody made is a claim
about another system's data.

**The observability stack enriches; it never owns.** `nest-rs-opentelemetry` mints
no identifier — its `IdGenerator` **adopts** the kernel's, so the exported trace
and the log lines name one request by one value rather than two. What it adds is
the remote parent link and the sampler's verdict. It owns neither the span, nor
the access log, nor the trace context.

**Propagation is the edge's obligation, at every boundary it crosses — task,
process, and response.**

- A framework `spawn` that does not carry the span roots its events at nothing —
  found at MCP, WS and the GraphQL dataloader, and presumed at any new one until
  checked.
- A queue crosses a *process*, so `traceparent` and `tracestate` travel in the
  wire envelope (`nest_rs_queue::envelope`) and the consumer continues from them,
  making the job a **child of the enqueue**. A value carrying none is a legacy or
  foreign payload and starts a trace, never a refusal.
- **A unit of work ends when its answer ends, not when its handler returns.** A
  streaming body — `#[sse]`, a download, anything over a `Stream` — runs
  afterwards on the connection task with the edge's task-locals unwound. The HTTP
  edge carries the request into that gap (`RequestContinuation`, re-installed
  around every body poll) and does so **whatever the access log is set to**: a
  config flag is a weaker condition than a crate, and a primitive true for short
  responses and false for long ones reads as present while being absent exactly
  where a long operation needs it.

**A continuation that outlives its request inherits identity, never resources.**
A socket opened by an upgrade may live for hours after its `101`, so it takes the
upgrade's trace and actor and nothing else: it already steps out of the request
transaction, and pinning that request's scope for the socket's life is the same
defect wearing another name. Where the framework dispatches *inside* the
connection, the operation is the unit and the connection is a field
(`ws.message` carries `ws.connection_id`); where it cannot see one, the
connection is the unit (`graphql.subscription`). Both lifecycle hooks are units
too — `ws.connect`, `ws.disconnect` — because a hook is developer code that logs
and writes like any handler.

**What the caller reads back is `traceresponse`**, the working group's response
form — standards-track rather than a Recommendation, and stated as such. It
carries the trace this service *actually used*, which at a front gate is
deliberately not the one the caller asked for.

**The span reports the route template, never the path as addressed.**
`http.route` is what a backend groups latency and error rates on, so
`/users/01a0…` there is one group per identifier and no signal at all; the
addressed path is `url.path`. And because `tracing` fixes a span's name to a
literal, the exported name comes from **`otel.name`** — `{method} {route}`,
recorded once poem's router has answered, and the method alone when nothing
matched. Naming an unmatched span after its URL is how one scanner fills a
tracing backend with junk, so that fallback is the conventions' and ours.

### What a log line carries — one unit of work, read off the context

**Every log line carries the W3C trace context of the unit of work that emitted
it** — `trace_id`, `span_id`, `actor_id` — and **carries no span state at all**:
not a span's attributes, not a span's name. The formatters are
`nest_rs_core::logging::{TextFormat, JsonFormat}`, they are the *same* two
whichever subscriber is mounted, and they read the ambient correlation rather
than the span scope. Three reasons, each already a shipped defect: a span's
fields belong to the span, so rendering them put an HTTP request's method, path
and client address on every line a service below it emitted; `tracing` renders a
whole scope, so a nested unit of work printed one `trace_id` per level; and the
context outlives the span, so a streaming body is readable exactly where a span
stack is not. **Never write those three as event fields** — that is the
duplicate, and it is what `#[deny]`-less code drifts back into.

Three deviations from OpenTelemetry's log data model, all deliberate, stated
here because the rule is that the answer is stated rather than identical:

- **`actor_id` is not in the model**, whose nearest equivalent is the `user.id`
  *attribute*. Ours is an audit identity with framework-wide vocabulary, carried
  as a top-level key beside the ids because that is where an operator greps it.
  Renaming it to `user.id` and demoting it into `fields` is the only conformant
  alternative, and it buys nothing operationally.
- **`trace_flags` is JSON-only.** The model names it beside the two ids, and a
  JSON record may be joined against an export where the sampling bit decides
  whether the other half exists. A human at a console never acts on it.
- **`parent_span_id` is a span field and never a line field.** The model has no
  ancestry, and a line has no need of one now that nothing renders a scope.

**Every edge files one line per unit of work, and that is what makes the rest
affordable.** The ids relate lines; the *identity* of the work — which route,
which event, which job, which tool — is an **event** attribute on a line the edge
emits once per unit, the way HTTP's access log always has. An edge without
that line leaves its work anonymous on the console, which is the state HTTP's
access log has always prevented and the other edges lacked.

**A unit of work has one canonical name, and it is `<edge>.<unit>`.** It is
declared once by **the crate that owns the edge**, as `<crate>::unit::<UNIT>`
(`nest_rs_http::unit::REQUEST`, `nest_rs_ws::unit::MESSAGE`, …), and read three
times — by the `operation_span!` that opens the unit, by the operation line's `name:` (the
event's metadata identity, which an OTLP log bridge exports as `event.name`),
and by that line's `message`. Two of the three used to be two vocabularies: the
spans said `http.request` and `mcp.operation` while the lines said `request
served` and `operation served`, so the set had to be learned twice and a query
could name the wrong one. Sharing a constant is what makes them agree; the
`units` join in `nest-rs-conformance` is what stops a fourth spelling, and it
fails on a literal at any such site, on a namespace outside the closed edge
vocabulary, and on a unit that opens a span but files no line.

**Every string the framework interprets is a constant, and a span target is
declared by the crate that owns the concern.** Never a literal at the call site: a typo
there is a target carrying one event that no filter selects, and nothing says
so. **The owning crate declares it** — `nest_rs_events::TARGET`,
`nest_rs_seaorm::TARGET` — because a target's one job is to name *where* an
event came from, so a central table in the kernel would mean `nest-rs-core`
holding a name for a concern it does not know exists. **Owns, not emits**: a
concern several crates emit on (`nest_rs::routes`, from five) is declared once
by the crate the others already depend on, and read from there. A crate owning exactly one
concern spells it `TARGET` at its root; a crate owning several gets a `target`
module (`nest_rs_core::target`, `nest_rs_http::target`, and those two only).
`operation_log::kind` holds the three `otel.kind` values this framework emits
— `server`, `consumer`, `internal`. The specification defines five; nothing here
opens a span for an outbound call or for handing work to a queue, and a constant
nothing fills is the same defect as a span field nothing records, so the other
two are added by whichever edge first needs one. A `*-macros` crate reaches the owner through **its own surface crate's**
re-export (`::nest_rs_queue::TARGET` from `nest-rs-queue-macros`), the form
`framework.md` sanctions — its expansion lands in crates that do not all carry
the umbrella. Reaching a *different* sibling is the breach that rule names.
`nest-rs-ws-macros` had two of them (`::nest_rs_core::target::LAYERS`,
`::nest_rs_http::target::ROUTES`); both are closed — it emits
`::nest_rs_ws::target::{LAYERS, ROUTES}` and `nest-rs-ws` re-exports the two
under its own `target` module, which is the sanctioned form. No `*-macros` crate
reaches a different sibling today.

**One target is the family's toggle, and that is why no edge grows a config for
it.** Every operation line is `nest_rs::operation`, so `<PREFIX>_LOG` silences
the whole family with `nest_rs::operation=off` and no other spelling exists.
`NESTRS_HTTP__ACCESS_LOG` stays because it predates the family and is an app's
pinned config rather than a deployment's filter — it names one edge's line, not
the family's; inventing a `#[config]` — and the `for_root` seam that one obliges
— for four more booleans would be four declarations of a decision the filter
already makes.

**That target names the line, and it is the only one that names anything but a
subsystem.** Every other is rooted at the crate that emits it; this one is a
category of line crossing all of them — one per unit of work, carrying an
`outcome` and a `duration_ms` — so a subsystem-shaped word reads as a subsystem
and hides what it is. It was `nest_rs::access` through 5.1, which was HTTP's
word for its own line surviving the generalisation to six edges: a clock tick
has no caller, so nothing accesses anything.

**No target may prefix another**, and that is mechanical rather than aesthetic:
`EnvFilter` matches a directive's target with `starts_with` on the raw string,
not on `::` segments. `nest_rs::access` prefixed `nest_rs::access_graph`, so the
documented toggle also silenced the boot `warn` naming unreachable GraphQL
resolvers — an operator lost a startup diagnostic with no sign of it. Derived
and executed by the `filters` join in `nest-rs-conformance`, which walks every
target both workspaces emit.

## Testing

Wiring bugs don't surface in unit tests.

**The devcontainer provides live backends — e2e infra is ALWAYS
reachable here.** Postgres (`postgres:5432`), Redis (`redis:6379`), S3
(`rustfs:9000`) are `depends_on: service_healthy`, up before you get a
shell; `demo/.env` wires those hostnames. **Never claim they are
unreachable and skip e2e on that basis** — a recurring, *false*
assumption (owner-confirmed 2026-07-09). A real connection failure is a
regression to report, not an environment limit.

**THE test-layout norm — locked 2026-07-09, do not reopen.** A finding
that seems to justify a change goes to the owner as a *question*, never
as an edit.

1. **A test target is always a directory: `tests/<suite>/main.rs`** —
   even for one file. A flat `tests/<x>.rs` is forbidden: Cargo compiles
   it as its own binary, escaping the nextest gates and relinking per
   file.
2. **Exactly two legal suite names.** `integration` — the crate's public
   API in process, no DB/network. `e2e` — needs live infra, gated by the
   nextest filter `binary(e2e)`, **never** `#[ignore]`.
3. **Inside the suite the module tree mirrors `src/`.** `main.rs` is the
   suite *root*, never a test module: `//!` + the `mod` list + the
   fixtures the siblings share (`crate::…`) — **no `#[test]` function
   lives there.** A test belongs to the module named for the `src/`
   concern it covers, so "where is this asserted?" has the same answer as
   "where is this implemented?". One exception: `nest-rs-testing`
   organizes by concern.
4. **Unit tests are untouched** — `#[cfg(test)] mod tests` in the file
   under test.
5. **The runner is nextest.** Bare `cargo test` is unsupported except
   `--doc`.

## Definition of done

Only call a task done when these pass for **every workspace touched**.
Report what ran; never claim a step you skipped. **Show evidence, don't
assert success.**

**Framework (root workspace)** — no `Justfile` here, cargo directly:

```
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
cargo nextest run --workspace -E 'not binary(e2e)' && cargo test --workspace --doc
cargo nextest run --workspace -E 'binary(e2e)'   # if it touches seaorm/storage
cargo audit                                      # if a manifest or the lockfile moved
```

**Product (`demo/`)** — `nestrs run` is the single front door:

```
nestrs run lint
nestrs run test unit
nestrs run test e2e      # if it touches transports, DI wiring, or persistence
```

**HTTP/GraphQL changes need one more step**: run the binary, `curl` the
affected endpoints, confirm the response, then **kill the server before
returning control**. Tests passing is not evidence a route is mounted.

GraphQL apps commit their SDL (`apps/<app>/schema.graphql`), regenerated
as a side effect of the dev run — no standalone generator, no CI check.

## Autonomous work — stop and ask

These are owner decisions. In a `/loop` or unattended run, **halt and
surface the question** rather than pick:

- Anything on the *Hard "no" list* that the task appears to require.
- Reopening a locked decision (test layout, workspace split, crate
  naming).
- A new third-party dependency.
- A second way to do something a decorator already does.
- A migration that drops or rewrites existing data.
- A documented rule that has drifted from the code — report it; don't
  edit either side to match.

**Progress rule:** if two consecutive iterations make no measurable
progress against *Definition of done*, stop and report the blocker
instead of trying a third variation.

## Workflow

State the plan in one or two sentences before tools. Batch independent
calls in parallel. Run the *Definition of done* sequence for every
workspace you touched. Report what changed and what was verified — no
paragraph-long summary.

**Green tests are not evidence, and `/audit` is how that gets checked.**
A suite written alongside a change is blind exactly where the change is:
it finds what its author thought of. Run `/audit` before calling a
non-trivial change done, and **again after fixing what it found** — the
fix is new code nobody has read. It fans narrow lanes out to agents whose
mandate is to *prove and not fix*, ranks silence above noise, and
separates "clean" from "not looked at". The classes it hunts are the ones
that have actually shipped here; the skill carries them.

**Three review skills, and the order is what a mistake costs.**
`/architecture` asks where a thing lives and under what name; it takes a
**scope** — a crate, a subsystem — never a diff, so it runs when a territory
opens (a new crate, a new decorator family, new vocabulary, a new `for_root`
seam) or when a scope has drifted over several sessions, not per commit.
`/audit` asks whether the code answers wrongly, on the working tree.
`/simplify` asks whether the shape is heavier than the job, on the current
diff. When more than one applies, run them in **that order**: a move
invalidates every probe taken before it, and a probe invalidates the polish.
Two of the three write — `/architecture` applies what a written rule already
decided, `/simplify` applies its own findings — so both are followed by the
*Definition of done* and, for anything non-trivial, by `/audit`, since applied
code is new code nobody has read. `/audit` itself never writes: it proves, and
hands the fix back.

## Reading order

This file plus the **code** are the source of truth.

1. **This file** — durable rules.
2. **`demo/crates/features/src/users/`** — reference feature; copy
   before inventing. If the copy isn't enough, fix the exemplar — don't
   invent a second pattern.
3. **`demo/apps/api/module.rs`** — canonical composition.

User-level IDE rules (e.g. "explain in French, code/comments in
English") apply per session.
