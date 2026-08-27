# NestRS documentation site

The public documentation — [nestrs.dev](https://nestrs.dev) — built with
[Astro](https://astro.build) 7 + [Starlight](https://starlight.astro.build).
Source lives next to the code so a PR that changes an API can update the doc in
the same commit.

## Run locally

Requires **Node.js 22.12 or newer** — Astro 7's own floor, and what
`package.json`'s `engines` declares. CI and the devcontainer both run **Node 24**
(the active LTS), pinned in `.github/workflows/docs-pages.yml` and
`.devcontainer/Dockerfile`; that is the version a build is proven against.

```bash
cd docs
npm install
npm run dev        # → http://localhost:4321
npm run lint:docs  # the style & code-truth gate — CI runs this before the build
npm test           # the linter joined against itself
npm run build      # static site under docs/dist/
```

`npm run build` produces a fully static tree (HTML, CSS, minimal JS, a static
search index, and the `llms.txt` family). `npm run preview` serves it.

## What lives where

| Path | What it is |
|---|---|
| `src/content/docs/` | every page — one directory per section |
| `STYLE.md` | **the law** for docs prose and structure; read it before editing a page |
| `templates/` | the five skeletons `STYLE.md` §B names — T-CONCEPT, T-INDEX, T-TUTORIAL, T-RECIPE, T-SINGLE |
| `scripts/lint-docs.mjs` | the gate: every rule in §F, plus the H2 vocabulary, the caps, links and anchors |
| `scripts/lint-baseline.json` | violations a rule inherited when it landed — shrinks only |
| `canon.json` | **generated** — the framework facts the linter checks pages against |
| `demo-sources.json` | **generated** — every `demo/` file a fence may quote, with the port it pins |
| `src/sidebar.mjs` | the Basics / All options tier split — threshold, vocabulary, exemption |
| `src/components/Sidebar.astro` | the menu: two levels, and only the section you are in lists its pages |
| `src/redirects.mjs` | one entry per route that ever shipped and moved |

**The two `*.json` are written by `cargo nextest run -p nest-rs-conformance`, never
by hand.** They are what lets the linter derive nothing: it opens no file outside
`docs/`, so the workflow's `docs/**` path filter is an exact declaration of the
job's input set. A framework change that moves a documented fact regenerates the
canon, lands a `docs/**` diff, and trips the docs job on the commit that caused it.

## Editorial rules

`STYLE.md` is the single source of truth; these three are the ones a session gets
wrong first.

1. **Never repeat — link.** Every concept has exactly one canonical page. Other
   pages get one sentence and a link.
2. **Every code example must compile, and every one says which file it is.**
   Every path is workspace-shaped, because that is what `nestrs new` produces
   by default: `crates/features/src/<module>/<role>.rs` for feature code,
   `apps/<app>/src/…` for composition. No page mixes in the `--standalone`
   `src/…` shape. A block quoting the demo says so in words —
   `(from the demo)`, `(from the demo, abridged)` — and that marker, not the
   path, is what makes it an excerpt the linter checks line by line. §F lists
   what the linter greps for, each rule filed against a shipped release by a
   reader following a page verbatim.
3. **A "Why this design" subsection on every non-trivial concept.** NestRS's
   value is in the *decisions* — make them legible.

## Sections

**The menu is two deep and never three**: a group is a section, its items are
that section's pages. Sixteen doors, read as a path — and the tutorial sits
second because the fastest way into the framework is to build something.

```
Start here      index, why, why-not-axum, benchmarks, coming-from-nestjs,
                getting-started, cli, publish
Tutorial        build a posts feature end to end
Fundamentals    architecture, fundamentals/
Configuration   configuration/
HTTP            http/, openapi
GraphQL         graphql/
WebSockets      websockets/
MCP             mcp/
Data            database/, storage
Security        overview, two guides, threat-model
Authentication  security/authentication/
Authorization   security/authorization/
Background work queue/, schedule, events
Testing         testing/
Operations      opentelemetry/, server-timing, health/, rate-limiting
Reference       packages, decorators, glossary
```

A group whose pages are one directory is `autogenerate`d from each page's
frontmatter `sidebar.order`, so adding a page touches no config. A group
gathering several directories lists them in reading order, and `Security` names
its four pages by slug — autogenerating it would nest `authentication/` under it,
which is the third level again.

A section of **five or more non-index pages** presents two lists — **Basics**
then **All options** — in its index's "In this section" prose. A page declares
which one it is in with `tier:` in its frontmatter. That split is *not* a menu
level: it is a reading of one section, offered where the reader of that section
already is. `tutorial/` is exempt at any size: its pages are steps 1..n, so a
tier boundary mid-sequence would claim something false. `STYLE.md` §G is the
norm, `src/sidebar.mjs` owns the threshold, and the linter gates it.

## Deploying

GitHub Pages, from `.github/workflows/docs-pages.yml`, on every push to `main`
touching `docs/**`: `npm ci` → `npm run lint:docs` → `npm run build` with
`ASTRO_SITE=https://nestrs.dev` and `ASTRO_BASE=/` → `deploy-pages`. There is no
other docs CI, and the lint step gates the deploy.

The output is a plain static tree, so publishing it anywhere else is
`npm ci && npm run build` from `docs/` and serving `docs/dist/` — set
`ASTRO_SITE`/`ASTRO_BASE` to match the host, or absolute URLs and the sitemap
will point at nestrs.dev.
