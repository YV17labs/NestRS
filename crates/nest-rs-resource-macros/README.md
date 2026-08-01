# nest-rs-resource-macros

Macro crate for the NestRS framework — the #[expose] attribute (SeaORM entity to GraphQL + OpenAPI); re-exported by nest-rs-resource: depend on that crate, not this one.

A `proc-macro` companion crate — never added directly. Its decorators are re-exported by the surface crate, reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

[Documentation](https://nestrs.dev/tutorial/entity/) · [GitHub](https://github.com/YV17labs/NestRS)
