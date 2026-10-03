---
paths:
  - "docs/src/**"
  - "docs/templates/**"
  - "docs/*.md"
  - "docs/package.json"
  - "docs/astro.config.mjs"
  - "crates/*/README.md"
---

# Docs site — STYLE.md is the law

`docs/STYLE.md` is the source of truth for docs prose and structure — **read
it before writing or editing a page**, then start from the matching skeleton
in `docs/templates/` (T-CONCEPT, T-INDEX, T-TUTORIAL, T-RECIPE, T-SINGLE). On
prose, `STYLE.md` wins; on code or naming, `CLAUDE.md` does. This file holds
only the traps a session hits before it thinks to look.

## What a page owes the code

No script checks a page against the code: the author does, before it ships,
with `STYLE.md` § F as the list of what has shipped wrong. The code wins a
disagreement, and the page is fixed in the same commit as the code it follows.

- **A capability's `## Install` and its crate README spell
  `cargo add nest-rs --features <x>`**; every unit of work, span target and
  capability is named on some page.
- **A `nest_rs::…` or `nest_rs_<crate>::…` path a page or a crate README names
  resolves to a public item**; a sentence about a private module names its
  file (`crates/<crate>/src/<module>.rs`) instead — the `paths` check in
  `nest-rs-conformance`.
- **A datastore key a page spells is one the code declares** — the `keys`
  check in `nest-rs-conformance`.
- **The `/architecture/` page agrees with the role tables and the reserved
  vocabulary of the shipped `architecture.md`.**
- **A capability is a feature a developer can type after `--features`**, not a
  crate.

## Gotchas no page shows

- **Snippets are hand-written** — there is no extraction from `examples/`. A
  fence title carrying `(from the demo)`, `(from the demo, abridged)` or
  `(from the demo — …)` claims an excerpt: the named demo file exists and every
  line not elided with `// …` appears in it, in order.
  A title without the marker is an illustration and asserts nothing. Titles
  use the developer's workspace shape (`crates/features/…`); GitHub URLs use
  the real repo path (`demo/crates/features/…`).
- **A snippet with no counterpart in `demo/` or the owning crate's suite is
  undocumented** (`manifests-ci.md`, *Shipping a capability*).
- **`npm run build` is the docs' one check**, run by CI's `docs` job on every
  pull request touching `docs/**`. Deploy is `docs-pages.yml` on push.
