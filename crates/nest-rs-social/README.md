# nest-rs-social

Open social-login provider contract for NestRS — a flow-owning SocialProvider trait, an inventory-discovered registry, and first-party GitHub/Google providers. Third-party providers plug in as independent crates through the same public seam.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features social
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/security/authentication/social-login/) · [GitHub](https://github.com/YV17labs/NestRS)
