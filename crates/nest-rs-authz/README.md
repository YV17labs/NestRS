# nest-rs-authz

CASL-style authorization for NestRS: one ability definition driving an access gate, a SeaORM query pre-filter, and response field-masking. Transport bindings (`http`, `graphql`, `mcp`) live behind Cargo features; the database-coupled extractors (`Bind`, `bind`, `LoaderScope`, `WsDataContext`) live in `nest-rs-seaorm` so the engine stays free of a data-layer dependency.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-authz = "1.0"
```

[Documentation](https://nestrs.dev/security/authorization/) · [GitHub](https://github.com/YV17labs/NestRS)
