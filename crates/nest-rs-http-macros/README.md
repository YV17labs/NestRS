# nest-rs-http-macros

Macro crate for the NestRS framework — HTTP decorator macros (#[controller], #[routes], verb attributes, #[interceptor]); re-exported by nest-rs-http: depend on that crate, not this one.

A `proc-macro` companion crate — never added directly. Its decorators are re-exported by the surface crate, reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

[Documentation](https://nestrs.dev/http/) · [GitHub](https://github.com/YV17labs/NestRS)
