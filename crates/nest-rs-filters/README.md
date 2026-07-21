# nest-rs-filters

Transport-spanning error-mapping filters for NestRS — one Layer System sub-trait that turns inner errors into responses on HTTP, GraphQL, and WS, declared globally via `use_filters_global` or per-scope through `#[use_filters]`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-filters = "1.0"
```

[Documentation](https://nestrs.dev/fundamentals/middleware/) · [GitHub](https://github.com/YV17labs/NestRS)
