# nest-rs-authn-macros

Macro crate for the NestRS framework — #[derive(PrincipalIdentity)] (a claims type's subject, tenant and scopes, read off the fields it marks); re-exported by nest-rs-authn: depend on that crate, not this one.

A `proc-macro` companion crate — never added directly. Its decorators are re-exported by the surface crate, reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

[Documentation](https://nestrs.dev/security/authentication/jwt/) · [GitHub](https://github.com/YV17labs/NestRS)
