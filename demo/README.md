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
| `nestrs run start <app>` | Run an app in release mode, under the production profile, e.g. `nestrs run start api`: it reads no development file and no app's own `apps/<app>/.env`, so it names its database, Redis, store and port on the process, as a deployment does, an app verifying tokens, every app but the worker, its verifying key, and `auth` its signing key, clients and sign-in resources. Against the dev container's, declare development: `NESTRS_ENV=development nestrs run start api` |
| `nestrs run build <app>` | Build one app in release (default `api`), e.g. `nestrs run build live` |
| `nestrs run build --all` | Build release binaries for every app in the workspace |
| `nestrs run test unit` | Run unit + integration + doctests (no DB) |
| `nestrs run test e2e` | Run e2e tests (Postgres required) |
| `nestrs run test cov` | Run coverage on the full suite |
| `nestrs run test doc` | Run doctests only (`///` examples) |
| `nestrs run lint` | Clippy (strict) + format check |
| `nestrs run chart` | The Helm chart's lint and render cases: what each app's Secret holds, and each install it refuses (needs `helm` and `yq`) |
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

Each app's port, like the audience it requires, is its own setting, in
`apps/<app>/.env`: read over the shared `.env*` files under the development and
test profiles, and set by the deployment otherwise.

| App | Kind | Port |
|-----|------|------|
| `auth` | OAuth2 / JWT token issuer | 3001 |
| `api` | REST + GraphQL + OpenAPI, persisted & authorized | 3002 |
| `assistant` | Model Context Protocol server | 3003 |
| `live` | Real-time WebSocket gateway | 3004 |
| `worker` | Background jobs & scheduling (health only over HTTP) | 3005 |

Every app opens Postgres — run `nestrs run db up` once first (or
`nestrs run db reset` to also load demo users). `api`, `assistant` and `worker`
also open Redis, and their audio routes and jobs read and write the object
store.

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
docker run --rm -p 3002:3002 -e NESTRS_HTTP__PORT=3002 nestrs                       # default `api`
docker run --rm -p 3001:3001 -e NESTRS_HTTP__PORT=3001 nestrs /usr/local/bin/auth  # any other binary
docker run --rm nestrs /usr/local/bin/migrate up                                   # apply migrations
```

The image runs under the production profile, which reads no app's own
`apps/<app>/.env`: each container names its port, as the chart does.

Runtime image is `gcr.io/distroless/cc-debian13:nonroot` — no shell, no package
manager, runs as UID 65532. `cargo-chef` cooks dependencies in a cacheable
layer. Both builds are `--locked`, so a `Cargo.lock` behind its manifests fails
the image, and run on the toolchain the `RUST_VERSION` argument names. Adding a
new app under `demo/apps/` requires no Dockerfile change.

`migrate` ships beside the apps — the schema is an app of its own. `seed` does
not: it writes demo fixtures, which a production image has no business being
able to do, so it stays the local `nestrs run db seed`.

## Kubernetes

[`charts/demo/`](charts/demo/) is a Helm chart over that same image. Every
app is one entry in `apps`, and `kind` says what it becomes — a Deployment
behind a Service, or a job that runs to completion before them:

```bash
openssl genpkey -algorithm ed25519 -out signing.pem
openssl pkey -in signing.pem -pubout -out verifying.pem

helm install demo charts/demo \
  --set apps.auth.host=auth.example.com \
  --set apps.api.host=api.example.com \
  --set apps.assistant.host=assistant.example.com \
  --set apps.live.host=live.example.com \
  --set secrets.SEAORM__URL="postgres://user:pass@db:5432/nestrs" \
  --set secrets.REDIS__URL="rediss://redis:6379" \
  --set secrets.REDIS__TRANSCODES__URL="rediss://cache:6379" \
  --set secrets.STORAGE__ACCESS_KEY="$STORAGE_ACCESS_KEY" \
  --set secrets.STORAGE__SECRET_KEY="$STORAGE_SECRET_KEY" \
  --set-file secrets.AUTHN__PRIVATE_KEY=signing.pem \
  --set-file secrets.AUTHN__PUBLIC_KEY=verifying.pem \
  --set-file secrets.OAUTH__SERVER__CLIENTS=clients.json
```

The secrets are the *Required* list in [`values.yaml`](charts/demo/values.yaml): a cluster's own services, the
object store's credentials, the signing keypair, and `clients.json`, the array of clients
`auth` registers, each with its `client_id`, `client_secret`, `payload`, `scopes` and
`resources`. The chart derives the cross-app URLs from the hostnames, and each app's
audience from its own. Leave out a secret, or the hostname of `api`, `assistant`, `auth`
or `live`, and the install is refused, naming each setting and the apps that cannot run
without it: an app reading one refuses its boot without it, but for the signing key,
without which `auth` boots and its token endpoint answers `500`. An app kept off the
Ingress pins its `AUTHN__AUDIENCE` in its own `config` instead. Social sign-in stays optional: a provider is on once
a deployment names its `CLIENT_ID` and `CLIENT_SECRET`, and one of the two alone is
refused like a missing secret.

Each app has a Secret of its own, holding the keys of `secrets` it reads and no other:
`auth`'s signing key and registered clients reach `auth` alone, and the migration job holds
the database URL. What an app reads is its `reads` in `values.yaml`, read off its modules; a
key no app reads refuses the install. A key changed rolls every app reading a Secret the
chart writes, under one checksum of them all: a pod is read where a Secret is not, and a
checksum of one app's Secret alone would test a guess at the secret it holds. To bring your
own, `apps.<name>.existingSecret` hands one app a Secret of yours in place of the chart's,
and `defaults.existingSecret` every app the same one.

Set `apps.<name>.host` rather than the cross-app URLs: the issuer, the audience,
the RFC 9728 resource identifier and the MCP `Host` allowlist are each some
app's public origin, and the chart derives them from the hostnames.

The queue worker scales on queue depth through [KEDA](https://keda.sh), shipped
disabled — its processors are I/O-bound and each `#[process]` method runs at
most its `concurrency` jobs at once per replica, so CPU stays flat however deep
the backlog gets. The chart's [README](charts/demo/README.md) carries that
trade-off and the rest.
