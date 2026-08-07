## Names — four levels, and none overflows into the next

| Level | Named for | Appears as |
|---|---|---|
| **Project** | the product | the repository and the workspace — **nowhere else** |
| **Crate** | what it holds | its directory, and the root of every span target it emits |
| **App** | what it **serves** (`api`, `worker`, `auth`) | the binary, and `<App>Module` |
| **Module** | its **domain** (`users`, `billing`) | `<module>/`, `<Module>Module` |

```
<App>Module              the app's own module.rs     composition root
<Module>Module           <module>/module.rs           the port
<Module><Edge>Module     <module>/<edge>/module.rs    one adapter
<WhatItBinds>Module      <name>/module.rs             a substrate
```

**No module or provider below the root ever carries the project's or the app's
name.** The project name stops at the workspace; the app name stops at
`<App>Module`. An app may share the project's name only while it is the only
app — and even then, nothing beneath it may.

**A module name is plural when the domain is a collection of enumerable things
(`users`, `orders`), singular when it is a capability (`auth`, `search`).** Not
cosmetic: the generator singularizes the folder name to derive the entity, so a
wrongly pluralized module produces a wrongly named entity, silently.

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
| Module config (`#[config]`) | `config.rs` |
| Domain error / Static constants | `error.rs` / `constants.rs` |

An adapter role carries its folder: `schedule/tasks.rs`, never `tasks.rs` at
the module root. A transport-specific guard belongs to its adapter too
(`mcp/guard.rs`).

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
