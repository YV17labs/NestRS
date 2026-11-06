---
paths:
  - "crates/nest-rs-cli/**"
---

# nestrs CLI — scaffolds mirror the exemplar

The command surface is in `nestrs --help`. Two boundaries are decisions:

- **`generate` has one adapter generator per edge** of the closed vocabulary
  (`architecture.md`), and a new edge is not shipped until its generator is.
- **`about` is the framework; `info` is the project.** `about` prints static
  lines identical on every machine; `info` reads the tree it stands in and says
  plainly when there is no project. A line that would read the same everywhere
  belongs to `about`, and the converse to `info` — which keeps the two from
  becoming one command with a flag.

## One starter — locked, do not reopen

**`nestrs new` has no template flag.** Every layout writes the same `hello`
module: a service with a greeting and one `#[public] GET /`. A new project must
prove it started, and a `404` proves nothing to the developer at a browser, so
there is no routeless variant; adding one back is a regression.

It is written as a **feature named after the app**
(`crates/features/src/<app>/`), because an app crate holds no `service.rs` or
`controller.rs` (`apps.md`). `nestrs new <name>` refuses when a feature already
owns that name.

## Templates

Templates are `const` strings with `{{placeholder}}`s in `src/templates/`,
rendered and wired by `src/scaffold/`, which rolls back a partial scaffold.

**A template becomes a file only when a second consumer must read the same
bytes.** There is one: `src/templates/architecture.md`, embedded with
`include_str!` into every scaffold's `AGENTS.md` and into `naming.rs`;
`.claude/rules/architecture.md` is this repository's copy, edited with it.
**Two readers parse it**: `naming.rs` derives the reserved words from the fence
under `## Reserved vocabulary`, and the scaffold test asserts its headings; the
`/architecture/` page restates its tables and that fence, kept in step by
review. Change a table, a heading or the fence with them.

**A template is the developer's own repository, so it may teach in a comment**
— the note above the GraphQL adapter's generated query, naming the grant it
waits for, is the case. A generated route or gateway is guarded, and a
generated GraphQL operation or MCP tool is gated on its port's entity or the
`#[subject]` the generator writes beside a port with none (`subject.rs`, placed
by `architecture.md`), so an opening is always the developer's line, never the
generator's. **What a port holds is read off its source, in every shape
`architecture.md` allows**: its `CrudService` in `service.rs` or `services/`, the
file of the port its `type Entity` leads to (`entity.rs`, `entities/<stem>.rs`), a
flat `Entity` from `mod.rs`; what the source leads nowhere, or to no attribute to
flag, is the run's next step, and a source that does not parse refuses the run by
its line, never a skip in silence. A grant a next step prints names its subject by
the path `authz/ability.rs` resolves. `g entity` retires a subject holding the
entity's name, which its re-export would shadow, while it is what the generator wrote —
the operations gated on it re-gated on the entity, which fails closed, and
their grant printed, never written — and refuses, naming the file, one edited
beyond that. Prose the framework compiles into behaviour is still an argument
there, as in `demo/` (`demo.md`).

## Scaffolds emit exactly what the rules mandate

**A scaffold writes the `clippy.toml` holding what no type can**: raw SQL, as the
repository's does (`data-layer.md`), so a developer's `nestrs run lint` refuses it
as `just lint` does here. Only what a developer's own code can write goes in it, so
a framework-internal entry stays the repository's. `crates/migrations` holds an empty
one replacing it there, so a generated migration's `Expr::cust("uuidv7()")`, and a
developer's DDL, take no `#[expect]`.

Templates stay in lockstep with the `users/` exemplar and the layout rules
(`features.md`, `apps.md`, `architecture.md`). Changing the exemplar or a
naming rule updates the matching template in the same change, and the
converse. A generator emitting a layout the rules forbid is a defect on a par
with breaking the exemplar.

**What generators write about one thing agrees, unedited.** One column list
writes the entity's fields, its table's migration and the round trip that
posts and reads back each column, and `g migration` reads an entity's
columns off its `Model` rather than asking for them twice: a scaffold whose
own files contradict each other fails the developer before a line is theirs.
Held by the integration suite's resource and migration tests, the scaffold
suite's every-type compile and the e2e replay.

**Every generator is compiled, not only read.** The scaffold suite runs every
adapter generator — every edge `Transport` carries — over both port shapes,
from the root of a one-app workspace, which wires that app, so the edits it
makes to that app's `module.rs` and manifest compile too, plus `nestrs new`'s
second app, `g migration` and `g entity`'s retirement of a subject, and holds
the result to the scaffold's own `clippy -D warnings`; `g auth`, `g graphql` and
`g mcp` also compile with `seaorm` off the root, the features crate alone, since
`migrations` and `seed` read it there. Its e2e half replays
every tutorial page that edits `blog` from the workspace root — `--app` naming
the app, as a reader with two apps types it — and runs the suite they leave
against the live Postgres, beside what booting alone proves: a call through a
generated gateway and tool, a key in the store the environment names, and each
shape of key a generated migration writes, applied and written to. A page's block
either marks the lines the reader writes or reads a file a generator wrote, so a
generator that stops writing one fails its page. The integration suite's text
assertions are not that proof: they read a wrong import as readily as a right one.

**A generator wires what it wrote into the app that serves it**, from
anywhere in the workspace, writing in one edit every import its code needs
that the root does not name — the backends the root chooses included, and the
table's migration: a next step the generator could have taken is a defect
(`decisions/generator-wires-its-app.md`). The integration suite holds each
generator to it.

**Scaffolded span targets are app-style** (`features::<snake>`), never
`nest_rs::*`: generated code is the developer's, not the framework's.
