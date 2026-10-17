# nest-rs-testing

In-process testing harness for NestRS: boot an app's real DI graph, headless or serving HTTP/GraphQL/MCP through its in-process TestClient (the `http` feature), with provider overrides and no socket bound.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add --dev nest-rs --features testing
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/testing/) · [GitHub](https://github.com/YV17labs/NestRS)
