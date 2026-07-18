# Publish — the NestRS demo

This is the **product** workspace: the runnable [NestRS](https://nestrs.dev)
apps plus the product crates (`features` / `migrations` / `seed`). It is a
*consumer* of the framework — the `nest-rs-*` crates live one level up in
[`../crates/`](../crates/) and are referenced by relative path in
[`Cargo.toml`](Cargo.toml). Work from **inside this directory**: `cd demo`,
then everything below resolves here (its own `Cargo.lock` / `target/`, the
`.env` cascade, and the `nestrs run` recipes).

> Building the framework on its own? That's the root workspace one level up —
> see [`../README.md`](../README.md).

## Quick start

From the dev container (Postgres + Redis already up):

```bash
cd demo
nestrs run db up          # apply migrations (api/auth need Postgres)
nestrs run dev api        # watch-mode API on http://localhost:3002
```

`nestrs run` forwards to the recipes in [`Justfile`](Justfile) /
[`db.just`](db.just) / [`test.just`](test.just); run it with no arguments to
list every recipe.

## Commands

| Command | What it does |
|---------|--------------|
| `nestrs run dev <app>` | Run an app in watch mode (rebuild + restart on change), e.g. `nestrs run dev api` |
| `nestrs run start <app>` | Run an app in release mode, e.g. `nestrs run start api` |
| `nestrs run build <app>` | Build one app in release (default `api`), e.g. `nestrs run build live` |
| `nestrs run build --all` | Build release binaries for every app in the workspace |
| `nestrs run test unit` | Run unit + integration + doctests (no DB) |
| `nestrs run test e2e` | Run e2e tests (Postgres required) |
| `nestrs run test cov` | Run coverage on the full suite |
| `nestrs run test doc` | Run doctests only (`///` examples) |
| `nestrs run lint` | Clippy (strict) + format check |
| `nestrs run fmt` | Apply rustfmt |
| `nestrs run check` | Fast type-check (no codegen) |
| `nestrs run db <verb>` | Manage the shared database: `up`, `down`, `fresh`, `status`, `seed`, `reset` |

`build --all`, `test` (with `e2e` / `cov` / `doc`), `lint`, `fmt` and `check`
operate on this whole workspace; `dev`, `start`, and `build` take an app name
(default `api`); `nestrs run db` (run bare to list the verbs) manages the
shared Postgres schema and seed data.

## The Publish workspace

**Publish** is a fictional multi-tenant publishing platform told through five
apps that share [`crates/features/`](crates/features/) and never RPC each
other. Full map: [nestrs.dev/publish](https://nestrs.dev/publish/).

| App | Kind | Port |
|-----|------|------|
| `auth` | OAuth2 / JWT token issuer | 3001 |
| `api` | REST + GraphQL + OpenAPI, persisted & authorized | 3002 |
| `assistant` | Model Context Protocol server | 3003 |
| `live` | Real-time WebSocket gateway | 3004 |
| `worker` | Background jobs & scheduling (headless) | — |

`api` and `auth` need Postgres; `worker` needs Redis — run `nestrs run db up`
once first (or `nestrs run db reset` to also load demo users). `assistant` and
`live` need neither.

The richest reference is `api`. Read it before inventing a second pattern —
copy it to start a new feature.

## Demo accounts

`nestrs run db reset` seeds two tenants (Acme, Globex), each with an admin and
members. Every account below shares the password **`publish-demo`**:

| Email | Org | Role |
|-------|-----|------|
| `admin@acme.test` | Acme | admin |
| `acme-user-1@example.test` | Acme | user |
| `admin@globex.test` | Globex | admin |
| `globex-user-1@example.test` | Globex | user |

## The magic moment (three curls)

With `nestrs run db reset` done and both `auth` (3001) and `api` (3002) running,
watch row-level filtering and field masking with nothing but curl:

```bash
# 1. Sign in as the Acme admin, capture the token.
ADMIN=$(curl -s localhost:3001/login -H 'content-type: application/json' \
  -d '{"email":"admin@acme.test","password":"publish-demo"}' | jq -r .access_token)

# 2. Admin sees every Acme user — emails included, and only Acme rows.
curl -s localhost:3002/users -H "authorization: Bearer $ADMIN" | jq

# 3. A plain member: same query, but `email` is masked out of every row.
MEMBER=$(curl -s localhost:3001/login -H 'content-type: application/json' \
  -d '{"email":"acme-user-1@example.test","password":"publish-demo"}' | jq -r .access_token)
curl -s localhost:3002/users -H "authorization: Bearer $MEMBER" | jq

# 4. Cross-tenant read is refused — a member's token cannot reach another org
#    (403). The admin here deliberately manages every publication, so the
#    isolation boundary is shown with the plain member from step 3.
curl -s -o /dev/null -w '%{http_code}\n' \
  localhost:3002/orgs/00000000-0000-7000-8000-0000000061b3 \
  -H "authorization: Bearer $MEMBER"
```

The handler never checks the tenant or the role — the framework carries it.

## Docker

The multi-stage [`Dockerfile`](Dockerfile) builds **every app binary** into a
single image. Because this workspace reaches the framework through
`../crates`, the build context must be the **parent** (`nestrs/`) — build from
one level up with `-f`:

```bash
cd ..                                                            # build from nestrs/
docker build -f demo/Dockerfile -t nestrs .
docker run --rm -p 3002:3002 nestrs                              # default `api`
docker run --rm -p 3001:3001 nestrs /usr/local/bin/auth         # any other binary
docker run --rm nestrs /usr/local/bin/migrate up                # apply migrations
```

Runtime image is `gcr.io/distroless/cc-debian13:nonroot` — no shell, no package
manager, runs as UID 65532. `cargo-chef` cooks dependencies in a cacheable
layer. Adding a new app under `demo/apps/` requires no Dockerfile change.
