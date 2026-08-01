# nest-rs-authz

Ability-based authorization for NestRS: one ability definition driving an access gate, a SeaORM query pre-filter, and response field-masking. Transport bindings (`http`, `graphql`, `mcp`) live behind Cargo features; the database-coupled extractors (`Bind`, `bind`, `LoaderScope`, `WsDataContext`) live in `nest-rs-seaorm` so the engine stays free of a data-layer dependency.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features authz
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/security/authorization/) · [GitHub](https://github.com/YV17labs/NestRS)
