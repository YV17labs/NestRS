---
paths:
  - "demo/apps/**/*.rs"
  - "demo/apps/**/*.toml"
  - "demo/**/*.just"
  - "demo/Justfile"
  - "demo/Dockerfile"
---

# Apps — pure composition

`demo/apps/<name>/` is **`main.rs` + `module.rs` only, by default**.
Not `examples/`, not `services/`.

`main` holds only `App::builder().module::<AppModule>()` (+ transports).
`module.rs` is the canonical composition — the app lists the edges it
serves.

## The exemplars

| App | Serves |
|---|---|
| `api` | REST + GraphQL + DB + authz — **the reference app** |
| `live` | WebSockets |
| `auth` | token issuer (signs; `api` only verifies) |
| `assistant` | MCP |
| `worker` | queue |

Simple hello/blog layouts are CLI-scaffolded only — see the docs, not
hosted in this repo.

## The app-local feature exception

A feature folder under `apps/<x>/` is the **exception**, not the norm:
glue handler over several features, or a deployment-specific route. Use
it only when *this app's exposure decides something the feature can't
generalize*; otherwise it belongs in `demo/crates/features/`.

Such an app-local feature **may flatten** — handler + `service.rs` +
`module.rs` at the folder root, no port/adapter split (`live/chat/`,
`assistant/weather/`). The hexagonal split is mandatory only in
`crates/features/`, where a slice must serve many apps and transports; an
app-local single-transport slice that only this binary uses keeps the
lighter layout.

## Multiple deployable apps

Splitting apps by responsibility is **a goal** (not microservices
sprawl), under two conditions:

1. **Share code through crates** — never copy-paste; product logic lives
   in `demo/crates/features/`.
2. **Keep coupling loose** — a self-contained token + a shared DB, never
   chatty RPC. `apps/auth` and `apps/api` share `crates/features` and the
   DB; they never call each other.

## Running the product

`cd demo` and drive it as its own repo. **`nestrs run` is the single
front door** — it forwards to `just`.

```
nestrs run dev [app]      # watch mode (default: api)
nestrs run start [app]    # release
nestrs run build [app]    # release build; --all for the workspace
nestrs run lint           # clippy -D warnings + fmt --check
nestrs run check          # fast type-check
nestrs run db <recipe>    # up|down|fresh|status|seed|reset
nestrs run test <kind>    # unit|e2e|cov|doc — bare `test` lists them
```

The `.env` cascade, `Justfile` / `db.just` / `test.just`, the `Dockerfile`
(built with the parent as context so it can reach `../crates`), and a
separate `Cargo.lock` / `target/` all live under `demo/`. The root
`.cargo/config.toml` (mold linker) is inherited hierarchically — **not
duplicated**.

**Note the asymmetry:** the root framework workspace has **no
`Justfile`** — verify it with bare `cargo` (see *Definition of done* in
`CLAUDE.md`).

## Transports

Every `HttpConfig` field is settable via `NESTRS_HTTP__*` env **and** the
pinned struct — the framework-wide **dual-path config rule**, which
applies to every `nest-rs-*` module.

An app activates a transport by importing its module
(`HttpModule::for_root(...)`, `QueueModule::for_root(...)`,
`OpenApiModule`, `OpenTelemetryModule`, …). There is no public
`.transport(...)` seam.

**Production output is OTLP, not stdout** — `nest-rs-opentelemetry` ships
the appender; the app opts in via `OpenTelemetryModule`. Dev
pretty-printing only under a `dev` profile.
