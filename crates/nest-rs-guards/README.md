# nest-rs-guards

Transport-spanning guards for NestRS — one trait, four transports (HTTP, GraphQL, WS, MCP), declared once with App::builder().use_guards_global(...).

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features guards
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/fundamentals/guards/) · [GitHub](https://github.com/YV17labs/NestRS)
