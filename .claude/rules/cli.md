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
— the `// SECURITY:` note above a generated `#[public]` route is the case that
decided it. Prose the framework compiles into behaviour is still an argument
there, as in `demo/` (`demo.md`).

## Scaffolds emit exactly what the rules mandate

Templates stay in lockstep with the `users/` exemplar and the layout rules
(`features.md`, `apps.md`, `architecture.md`). Changing the exemplar or a
naming rule updates the matching template in the same change, and the
converse. A generator emitting a layout the rules forbid is a defect on a par
with breaking the exemplar.

**Every generator is compiled, not only read.** The scaffold suite runs every
adapter generator — every edge `Transport` carries — over both port shapes,
from inside an app so the edits it makes to that app's `module.rs` and
manifest compile too, plus `nestrs new`'s second app and `g migration`, and
holds the result to the scaffold's own `clippy -D warnings`. The integration
suite's text assertions are not that proof: they read a wrong import as
readily as a right one.

**Scaffolded span targets are app-style** (`features::<snake>`), never
`nest_rs::*`: generated code is the developer's, not the framework's.
