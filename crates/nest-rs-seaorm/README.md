# nest-rs-seaorm

SeaORM integration for NestRS: the first-class implementation of the `nest-rs-database` extension contract. `DatabaseModule::for_root` owns the connection, composed at `App::builder()`. Transport extractors (`Bind`, `LoaderScope`, `WsDataContext`) live behind Cargo features.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-seaorm = "1.0"
```

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
