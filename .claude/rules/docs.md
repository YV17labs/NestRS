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
