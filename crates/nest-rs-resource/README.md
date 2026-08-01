# nest-rs-resource

Expose a SeaORM entity to REST/OpenAPI (wire DTO) from one declaration, via the #[expose] attribute; optional GraphQL surface behind the `graphql` flag + feature.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features resource
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/tutorial/entity/) · [GitHub](https://github.com/YV17labs/NestRS)
