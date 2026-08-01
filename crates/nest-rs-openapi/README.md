# nest-rs-openapi

OpenAPI 3.0 document generation and bundled Swagger UI for NestRS HTTP controllers (the REST analog of nest-rs-graphql's GraphqlModule).

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features openapi
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/openapi/) · [GitHub](https://github.com/YV17labs/NestRS)
