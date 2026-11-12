# nest-rs-database

ORM-agnostic data-layer port for NestRS: the unit of work every edge runs through (`UnitOfWork`, `run_unit`), and the ambient Executor its body reads, carried by the kernel's ambient context. SeaORM is the first-party implementation (nest-rs-seaorm), and the traits are public so other stores can be written against them.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features database
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
