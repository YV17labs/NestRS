# NestRS documentation site

The public documentation, built with [Astro](https://astro.build) +
[Starlight](https://starlight.astro.build). Source lives next to the code so
a PR that changes an API can update the doc in the same commit.

## Run locally

```bash
cd docs
npm install
npm run dev
# → http://localhost:4321
```

`npm run build` produces a fully static site under `docs/dist/` (HTML +
CSS + minimal JS + a static search index). Drop the folder on any static
host — Cloudflare Pages, GitHub Pages, OVH mutualisé via SFTP — no
runtime, no Node on the server.

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

```
index.mdx              Landing
why.mdx                The thesis + the six structural properties
getting-started.mdx    Install + first endpoint

tutorial/              Build a complete feature end to end (WIP)
concepts/              Modules, DI, access graph, ambient data context
http/                  Controllers, routes, guards, pipes, filters, interceptors
graphql/               Resolvers, fields, dataloaders, context bridge
websockets/            Gateways, messages, lifecycle
data/                  Entities, services, Repo, transactions, dataloaders
security/              Authn strategies, authz (Ability), masking, row-level
queue-schedule/        Durable jobs, cron, processors
mcp/                   Model Context Protocol tools
observability/         Telemetry, OTLP, Server-Timing, conventions
configuration/         Typed config from env + TOML, validation
testing/               nestrs-testing, overrides, e2e
```

Each section index follows the same four-tier shape: **Basics**,
**All options**, **Patterns**, **Internals**. Split into separate
files once a tier outgrows a single page (and update the section's
sidebar accordingly).

## Deploying

The site is fully static; any static host works. Quick paths:

- **GitHub Pages** — add a workflow that runs `npm ci && npm run build`
  and uploads `docs/dist/`.
- **Cloudflare Pages** — point the project at this repo with build
  command `cd docs && npm ci && npm run build` and output `docs/dist`.
- **OVH mutualisé** — `npm run build` locally (or in CI), SFTP `dist/`
  to `www/`.
