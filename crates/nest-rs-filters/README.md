# nest-rs-filters

Transport-spanning error-mapping filters for NestRS — one Layer System sub-trait that turns inner errors into responses on HTTP, GraphQL, and WS, declared globally via `use_filters_global` or per-scope through `#[use_filters]`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features filters
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/fundamentals/middleware/) · [GitHub](https://github.com/YV17labs/NestRS)
