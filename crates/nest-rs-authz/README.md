# nest-rs-authz

CASL-style authorization for nestrs: one ability definition driving an access gate, a SeaORM query pre-filter, and response field-masking. Transport bindings (`http`, `graphql`, `mcp`) live behind Cargo features; the database-coupled extractors (`Bind`, `bind`, `LoaderScope`, `WsDataContext`) live in `nest-rs-seaorm` so the engine stays free of a data-layer dependency.

Part of **[NestRS](https://nestrs.dev)** — an opinionated Rust framework for backend apps.

## Install

```toml
nest-rs = { version = "0.1", features = ["authz"] }
```

## Documentation

- [Authorization](https://nestrs.dev/security/authorization/)
- [Getting started](https://nestrs.dev/getting-started/)
- [Full documentation](https://nestrs.dev)

> **Alpha** — the public API is still evolving and this release is not production-ready.
