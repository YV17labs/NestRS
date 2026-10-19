# nest-rs-seaorm

SeaORM for NestRS: the adapter that wraps sea-orm. `SeaOrmModule::for_root` opens the one pool (`<PREFIX>_SEAORM__*`); `SeaOrmDatabaseModule` binds the `nest-rs-database` port over it (`SeaOrmUnitOfWork`, the unit of work every edge runs through, and `Repo` on its ambient executor); `#[expose]` and `#[wire_enum]` make an entity its wire DTO, inputs and GraphQL object. The HTTP route-model binder (`Bind`) lives behind the `http` feature; beside `health`, `SeaOrmHealthModule` gates the readiness and startup probes on the pool's `SELECT 1`, as `seaorm`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features seaorm
cargo add nest-rs --features seaorm,health   # the pool's readiness indicator
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
