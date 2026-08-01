# nest-rs-database

ORM-agnostic data-layer seam for NestRS: the ambient request/job Executor task-local plus the object-safe Executor trait an ORM module implements. SeaORM is the first-party implementation (nest-rs-seaorm), and the trait is public so other stores can be written against it.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features database
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
