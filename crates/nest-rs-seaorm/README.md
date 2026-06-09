# nest-rs-seaorm

SeaORM integration for nestrs: the first-class implementation of the `nest-rs-database` extension contract. `DatabaseModule::for_root` owns the connection, composed at `App::builder()`. Transport extractors (`Bind`, `LoaderScope`, `WsDataContext`) live behind Cargo features.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/NestRS/NestRS)
