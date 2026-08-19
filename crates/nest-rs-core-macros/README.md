# nest-rs-core-macros

Macro crate for the NestRS framework — the surface-agnostic decorators (#[injectable], #[module], #[hooks], #[input]). Re-exported by `nest-rs-core`: depend on that crate, not this one.

A `proc-macro` companion crate — never added directly. Its decorators are re-exported by the surface crate, reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

[Documentation](https://nestrs.dev/decorators/) · [GitHub](https://github.com/YV17labs/NestRS)
