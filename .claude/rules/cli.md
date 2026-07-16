---
paths:
  - "crates/nest-rs-cli/**"
---

# nestrs CLI — scaffolds mirror the exemplar

Command surface: `new` (monorepo / workspace app / `--standalone`),
`generate`/`g` (`feature`, `resource`, and the adapters `http` /
`graphql` / `ws` / `queue` / `schedule` / `mcp`), `run` (forwards to
`just` in the product workspace), `doctor`, `update`, `version`,
`about`.

## Scaffold architecture

Templates are `const` strings with `{{placeholder}}`s in
`src/templates/` (`feature`, `resource`, `adapter`, `workspace`,
`standalone`, `shared`). Rendering and auto-wiring live in
`src/scaffold/`: `render.rs` fills placeholders, `wiring.rs` performs
the two edits a copy can't (`features/src/lib.rs` `pub mod` line + the
module entry in the serving app's `module.rs`), `transaction.rs` rolls
back a partial scaffold.

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
