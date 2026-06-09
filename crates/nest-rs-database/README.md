# nest-rs-database

ORM-agnostic data-layer seam for nestrs: the ambient request/job Executor task-local + the object-safe Executor trait a third-party ORM module implements (sqlx, diesel, prisma-client-rust, mongo, …). The first-party SeaORM integration lives in nest-rs-seaorm; the row-level filter and response masker are SeaORM-specific by design.

[Documentation](https://nestrs.dev/database/) · [GitHub](https://github.com/NestRS/NestRS)
