# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the public API is still stabilizing (`0.x`), the minor version carries
both new features and breaking changes.

## [0.2.0] - 2026-06-10

### Added

- **CLI generators (`nest-rs-cli`).** New scaffolding binary with
  `nestrs g feature/resource/<transport>` — transactional scaffold core that
  generates files and auto-wires modules, with context detection.
- **`nestrs run` task front door.** Single entry point that forwards to `just`
  recipes, with first-run toolchain bootstrap (installs `just`, `bacon`,
  `cargo-nextest`, binstall-preferred; opt out via `--no-bootstrap` /
  `NESTRS_NO_BOOTSTRAP`).
- **Publish suite.** Exemplar workspace with org-scoped posts spanning REST,
  GraphQL, WebSockets, queue, and MCP apps.

### Changed

- **Unified layer pool.** Guards, pipes, interceptors, filters, and
  exception-filters now resolve through a single deduplicated pool per family
  (execute exactly once per request; broadest scope wins).
- **Apps renamed** and **service-naming conventions** tightened across the
  workspace (`svc` / `<name>_svc` injection naming).

### Fixed

- **Security: hardened authn/authz, transports, the data layer, and the CLI**
  against several edge cases.
- **Security: fail closed on unwired MCP** and **enforce a minimum HS256 secret
  length** at boot.
- Access-log `duration_ms` now rounded to microsecond precision.

### Documentation

- Added the Lifecycle fundamentals page and a dedicated packages page.
- Routed all task examples through `nestrs run`.
- Refined the splash hero / landing page (mobile layout, hello code-tabs demo,
  access-log terminal lines) and slimmed the README toward contributors,
  pointing users to nestrs.dev.

## [0.1.0] - 2026-06-08

Initial public release of the nestrs framework — an opinionated Rust framework
where the developer writes business logic and the framework carries the
cross-cutting concerns (authn, authz, row-level filtering, transactions, edge
validation, discovery, lifecycle).

### Added

- **Composition & DI.** Type-id container with `#[inject]` fields, `#[module]`
  composition, four-phase `App::builder().build()`, singleton/request/transient
  scopes, and a compile-time + boot-time access graph.
- **Request layers.** Guards, pipes, interceptors, filters, and exception
  filters with symmetric scopes (global / controller / handler) and TypeId
  dedup.
- **Transports.** HTTP (`nest-rs-http`), GraphQL (`nest-rs-graphql`),
  WebSockets (`nest-rs-ws`), queue (`nest-rs-queue` + `nest-rs-redis`),
  scheduler (`nest-rs-schedule`), MCP, and OpenAPI (`nest-rs-openapi`).
- **Authn / authz.** `nest-rs-authn` (strategies, `AuthGuard`, `JwtService`)
  and `nest-rs-authz` (abilities, ability guards, response masking) with
  bridges per transport.
- **Data layer.** `nest-rs-seaorm` with transparent ability-scoped `Repo`,
  ambient executor/transaction `task_local!`s, route-model binding, and
  auto-resolved GraphQL relations from `#[expose]`.
- **Supporting crates.** Pipes, events, health, throttler, config,
  opentelemetry, and the `nestrs` umbrella crate (`use nestrs::prelude::*`).
- **`nest-rs-*` naming alignment** across directories, packages, and imports;
  framework-owned error types.
- Rust 1.95 / edition 2024; tag-based release CI with the `mold` linker on
  Linux.

[0.2.0]: https://github.com/NestRS/NestRS/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/NestRS/NestRS/releases/tag/v0.1.0
