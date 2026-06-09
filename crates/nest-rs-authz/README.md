# nest-rs-authz

CASL-style authorization for nestrs: one ability definition driving an access gate, a SeaORM query pre-filter, and response field-masking. Transport bindings (`http`, `graphql`, `mcp`) live behind Cargo features; the database-coupled extractors (`Bind`, `bind`, `LoaderScope`, `WsDataContext`) live in `nest-rs-seaorm` so the engine stays free of a data-layer dependency.

[Documentation](https://nestrs.dev/security/authorization/) · [GitHub](https://github.com/NestRS/NestRS)
