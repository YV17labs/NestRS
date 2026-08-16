---
paths:
  - "docs/src/**"
  - "docs/templates/**"
  - "docs/scripts/**"
  - "docs/*.md"
  - "docs/package.json"
  - "docs/astro.config.mjs"
---

# Docs site — STYLE.md is the law

`docs/STYLE.md` is the single source of truth for docs prose and
structure — **read it before writing or editing any page**, then start
from the matching skeleton in `docs/templates/` (T-CONCEPT, T-INDEX,
T-TUTORIAL, T-RECIPE, T-SINGLE). This file carries only the traps a
session hits before it thinks to look.

On conflict about prose, `STYLE.md` wins; about code or naming,
`CLAUDE.md` wins.

## The caps the linter greps (CI-enforced)

- **≤ 3 Asides per page.** Reference page ≤ ~250–300 lines answering
  ONE question; tutorial ≤ ~250 lines ending on a runnable checkpoint.
- First screen: one working snippet ≤ ~15 lines, **no Aside above it**.
- Verbatim outputs real (run once, paste), trimmed to ≤ ~8 lines.
- Controlled H2 vocabulary in canonical order (`Install` → `Run it` →
  `Wire it in` → … → `Configuration` → `Limits` → `What fails if you
  get it wrong` → `Reference` → `Going further`); the closing block is
  `## Going further` (utility pages exempt).
- Banned words: *blazing(ly), powerful, seamless(ly), simply,
  effortless(ly), easy, magic(al)*. No exclamation marks in prose.
- **Example canon = the Publish universe only** (hello, blog/posts,
  users/orgs, chat/notify, audio, weather, media). The ban list
  (`items`, `products`, `artworks`, `file_assets`, `Ledger`, …) must
  stay at zero — never invent a feature.
- **Code-truth checks** — `version-pin`, `unauthed-curl`, `crud-error`,
  `bind-order`, `queue-name`, `install-stanza`, `otel-guard`,
  `decorator-import`, `layer-impl`, `exception-response-error`,
  `bare-log`, `config-table` (STYLE.md § F). Each was a shipped defect
  on a released page. `decorator-import`, `layer-impl` and
  `config-table` derive their rule from the framework's own source
  (every `#[proc_macro_attribute]` under `crates/*-macros/`, every
  `pub trait <T>: Layer` under `crates/`, a `#[config]` struct's fields)
  rather than restating it; `bare-log` is the docs half of the
  scaffold's own no-bare-log unit test.
- **`landing-claim`** — the one written before its defect rather than
  after it. The landing argues the framework with four figures, so all
  four are derived: capabilities from the umbrella's feature matrix,
  decorators from the decorator index, tests from the functions under
  `crates/`, pages from the content tree. Exact counts must agree; a
  `+` floor may lag inside a band and no further. Deleting a figure is
  deleting its gate, and the rule reports that too.

## Gotchas no page shows

- **Snippets are hand-written.** There is no `<Code file=…>` /
  `examples/` extraction (the docs README describes it aspirationally —
  unimplemented). A fence `title=` naming a real repo path must match
  the file **byte-for-byte** or say "(abridged)"; fictional snippets
  get generic `src/…` titles, never a real-looking path. Titles cite
  the user's workspace shape (`crates/features/…`); GitHub URLs use
  real repo paths (`demo/crates/features/…`).
- **The linter is baseline-gated**: `npm run lint:docs` fails only on
  violations not in `docs/scripts/lint-baseline.json`. Never run
  `--update-baseline` to silence a new violation — fix the page; the
  baseline only shrinks.

## Definition of done here

`cd docs && npm run lint:docs` — plus `npm run build` if you touched
config, components or styles. Deploy is `docs-pages.yml` on push; there
is no other docs CI.
