# nest-rs-seaorm

SeaORM integration for NestRS: the first-class implementation of the `nest-rs-database` extension contract. `DatabaseModule::for_root` owns the connection, composed at `App::builder()`. Transport extractors (`Bind`, `LoaderScope`, `WsDataContext`) live behind Cargo features.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs-seaorm
```

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
