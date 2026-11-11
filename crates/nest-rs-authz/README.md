# nest-rs-authz

Ability-based authorization for NestRS: one ability definition driving an access gate, a SeaORM query pre-filter, and the judgement of every wire row where it is built. Transport bindings (`http`, `graphql`, `ws`, `mcp`) live behind Cargo features; the loader that reads the row `#[authorize(A, bind = S)]` authorizes lives in `nest-rs-seaorm`, which owns the data layer it loads through.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features authz
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/security/authorization/) · [GitHub](https://github.com/YV17labs/NestRS)
