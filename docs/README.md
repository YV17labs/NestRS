# NestRS documentation site

The public documentation, built with [Astro](https://astro.build) +
[Starlight](https://starlight.astro.build). Source lives next to the code so
a PR that changes an API can update the doc in the same commit.

## Run locally

Requires **Node.js 22.12 or newer** (Astro 6).

```bash
cd docs
npm install
npm run dev
# → http://localhost:4321
```

`npm run build` produces a fully static site under `docs/dist/` (HTML,
CSS, minimal JS, and a static search index). Publish that directory with
whatever static hosting you already use — no application runtime required
on the server.

## Editorial rules

The three rules that keep the docs honest as the framework moves:

1. **Every "Basics" section ends with a link to "All options".** Readers
   coming back after a few weeks want the exhaustive page; do not hide it.
2. **Every code example must compile.** Snippets that are not lifted
   directly from a crate or app under this workspace should be moved to
   `examples/` and verified in CI before being included via
   `<Code file="..." />`.
3. **A "Why this design" subsection on every non-trivial concept.**
   NestRS's value is in the *decisions* — make them legible.

## Sections

Sidebar order follows the learning path — concepts before the capstone
tutorial, observability grouped without a "production" bucket:

```
Start here
  index.mdx, why.mdx, getting-started.mdx

Core (read before the tutorial)
  fundamentals/        Modules, DI, access graph, request layers
  configuration/       Typed config, .env cascade, ConfigSource
  http/                Controllers, routes, extractors, transport config
  database/            Entity, service, Repo, transactions, dataloaders
  security/            Authn, authz, row-level filtering, masking
  testing/             Unit, integration, e2e, policy tests

Capstone
  tutorial/            Build a users feature end to end

More transports
  graphql/, websockets/, openapi/

Background
  queue/, schedule/, events/, mcp/

Observability (dev + runtime insight — not a deploy-only chapter)
  opentelemetry/, server-timing/

Health, throttler       Probes and rate limiting (standalone sections)

Reference
  decorators.mdx, glossary.mdx
```

Each section index follows the same four-tier shape: **Basics**,
**All options**, **Patterns**, **Internals**. Split into separate
files once a tier outgrows a single page (and update the section's
sidebar accordingly).

## Deploying

The build output is a plain static tree. In CI or locally, run
`npm ci && npm run build` from `docs/`, then publish `docs/dist/` the same
way you would any other static site (object storage + CDN, static bucket
on a PaaS, rsync/scp to a web root, etc.).
