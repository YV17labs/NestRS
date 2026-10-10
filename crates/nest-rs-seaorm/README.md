# nest-rs-seaorm

SeaORM for NestRS: the adapter that wraps sea-orm. `SeaOrmModule::for_root` opens the one pool (`<PREFIX>_SEAORM__*`); `SeaOrmDatabaseModule` binds the `nest-rs-database` port over it (`Repo`, the ambient executor, the request layers); `#[expose]` and `#[wire_enum]` make an entity its wire DTO, inputs and GraphQL object. Transport extractors (`Bind`, `LoaderScope`, `WsDataContext`) live behind Cargo features.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features seaorm
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
