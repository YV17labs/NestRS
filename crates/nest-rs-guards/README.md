# nest-rs-guards

Transport-spanning guards for NestRS — one trait, four transports (HTTP, GraphQL, WS, MCP), bound on a host or a method with `#[use_guards]`, or app-wide by the root module with `GlobalGuards<(…)>`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features guards
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/fundamentals/guards/) · [GitHub](https://github.com/YV17labs/NestRS)
