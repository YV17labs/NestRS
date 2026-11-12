# nest-rs-seaorm

SeaORM for NestRS: the adapter that wraps sea-orm. `SeaOrmModule` opens the one pool (`<PREFIX>_SEAORM__*`); `SeaOrmDatabaseModule` imports it and binds the `nest-rs-database` port over it (`SeaOrmUnitOfWork`, the unit of work every edge runs through, and `Repo` on its ambient executor); `#[expose]` makes an entity its wire DTO, inputs and GraphQL object. The loader a route's `#[authorize(A, bind = S)]` runs lives behind the `http` feature; beside `health`, `SeaOrmModule` attaches the pool's indicator, so an app importing `HealthModule` gates the readiness and startup probes on its `SELECT 1`, as `seaorm`, with no import of its own.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features seaorm
cargo add nest-rs --features seaorm,health   # the pool's readiness indicator
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
