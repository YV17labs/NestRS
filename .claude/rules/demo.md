---
paths:
  - "demo/**"
---

# The product workspace — what binds all of `demo/`

`demo/` is the "Publish" demo and the framework's `sample/`: its own workspace,
consuming the framework by path, so a change spanning both compiles here
(`apps.md`, *Running the product*).

## No comments in the demo's Rust

The owner's rule, without exception: `demo/` Rust carries no `//`, `//!` or
`///` — in `apps/`, `crates/`, tests and `build.rs`. It is demonstration code,
and the shorter it reads the better it teaches; a line that seems to need a
comment is renamed or split, or its decision moves into the rules. A non-Rust
demo file — a chart, the `Dockerfile`, a `Justfile`, a manifest, `.env` — keeps
its *why* beside the value, in one short line
(`.claude/decisions/demo-comments.md`).

## Prose the framework compiles is an argument

A sentence the framework turns into behaviour is a decorator argument, never a
doc comment: `#[tool(description = "…")]` is what a language model reads to
choose the operation, and `#[api(summary = …, description = …)]` is the
OpenAPI document. In `demo/` the attribute form is the only form; the
framework's fallback to a doc comment (`macros.md`) is for consumers who write
comments.

## The `/why/` figure draws the demo's composition

`docs/src/components/Topology.astro` draws each app's feature modules and
edges, and the one path two of them share. An app added or gone, a change to an
app's `module.rs`, or a controller or gateway path moved updates the figure in
the same commit. Held by review, never by a script
(`.claude/decisions/modular-monolith-per-workload.md`).
