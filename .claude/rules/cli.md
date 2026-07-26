---
paths:
  - "crates/nest-rs-cli/**"
---

# nestrs CLI — scaffolds mirror the exemplar

Command surface: `new` (monorepo / workspace app / `--standalone`),
`generate`/`g` (`feature`, `resource`, `auth`, `migration`, and the
adapters `http` / `graphql` / `ws` / `queue` / `schedule` / `mcp`),
`run` (forwards to `just` in the product workspace), `doctor`, `update`,
`version`, `about`.

## One starter — locked, do not reopen

**`nestrs new` has no template flag.** Every layout writes the same
`hello` module (`src/templates/hello.rs`): a service with a greeting and
one `#[public] GET /`. A freshly created project must prove it started,
and a `404` proves nothing to the developer looking at a browser — so
there is no routeless variant, and adding one back is a regression.

Workspace mode writes it as a **feature named after the app**
(`crates/features/src/<app>/`), because the layout keeps no `service.rs`
/ `controller.rs` in an app crate; standalone writes the same two files
under `src/`. The service and controller templates are shared verbatim
between the two — only `{{service_use}}` differs. `nestrs new <name>`
refuses when a feature already owns that name.

## Scaffold architecture

Templates are `const` strings with `{{placeholder}}`s in
`src/templates/` (`hello`, `feature`, `resource`, `auth`, `migration`,
`adapter`, `workspace`, `standalone`, `shared`). Rendering and
auto-wiring live in `src/scaffold/`: `render.rs` fills placeholders,
`wiring.rs` performs the edits a copy can't (`features/src/lib.rs`
`pub mod` line + the module entry in the serving app's `module.rs`),
`transaction.rs` rolls back a partial scaffold.

**One `edit` per path per transaction.** `Scaffold::apply` resolves each
`edit` against the file *on disk*, so two edits of the same path both
read the original and the second write wins — fold the lines into a
single `ensure_lines`/dep list instead (see `g resource` bootstrapping
`g auth`). An `edit` on a file the same transaction `create`s fails: it
is not on disk yet.

## The lockstep obligation

**A scaffold emits exactly what the rules mandate.** Templates must
stay in lockstep with the `users/` exemplar and the layout rules
(`features.md`, `apps.md`, naming in `CLAUDE.md`). Changing the
exemplar or a naming rule ⇒ update the matching template in the same
task, and vice versa. A generator that emits a layout the rules forbid
is a defect on par with breaking the exemplar itself.

Scaffolded span targets use the app-name style (`features::<snake>`),
not `nest_rs::*` — deliberate: generated code is app code, not
framework code.
