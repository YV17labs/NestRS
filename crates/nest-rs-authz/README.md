# nest-rs-authz

Ability-based authorization for NestRS: one ability definition driving an access gate, a SeaORM query pre-filter, and response field-masking. Transport bindings (`http`, `graphql`, `ws`, `mcp`) live behind Cargo features; the extractors that load the row they authorize (`Bind`, `bind`) live in `nest-rs-seaorm`, which owns the data layer they load through.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features authz
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/security/authorization/) · [GitHub](https://github.com/YV17labs/NestRS)
