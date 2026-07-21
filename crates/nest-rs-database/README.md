# nest-rs-database

ORM-agnostic data-layer seam for NestRS: the ambient request/job Executor task-local plus the object-safe Executor trait an ORM module implements. SeaORM is the first-party implementation (nest-rs-seaorm), and the trait is public so other stores can be written against it.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-database = "1.0"
```

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/YV17labs/NestRS)
