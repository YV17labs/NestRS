# nest-rs-interceptors

The poem-typed interceptor of NestRS 6, which `nest_rs::core::Interceptor` replaced: nothing in the framework runs it, and the crate leaves in 7.0.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features interceptors
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/fundamentals/interceptors/) · [GitHub](https://github.com/YV17labs/NestRS)
