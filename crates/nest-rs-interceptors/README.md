# nest-rs-interceptors

Transport-spanning interceptors for NestRS — one Layer System sub-trait wrapping handler execution on HTTP, GraphQL, and WS, declared globally via `use_interceptors_global` or per-scope through `#[use_interceptors]`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-interceptors = "1.0"
```

[Documentation](https://nestrs.dev/fundamentals/interceptors/) · [GitHub](https://github.com/YV17labs/NestRS)
