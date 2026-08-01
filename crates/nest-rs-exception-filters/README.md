# nest-rs-exception-filters

Typed exception filters for NestRS — each filter declares the concrete error it claims and only catches matching errors; distinct from the unconditional error-mapping `Filter`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features exception-filters
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/fundamentals/exception-filters/) · [GitHub](https://github.com/YV17labs/NestRS)
