//! Compile-time witness of macro path hygiene (`framework.md`).
//!
//! This crate depends **only** on `nest-rs-*` surface crates — no third-party
//! dependency at all. Every decorator exercised here is therefore proven to
//! emit only `::std`/`::core` paths or paths routed through its surface
//! crate's re-exports: a bare third-party path (`::anyhow`, `::tracing`, …)
//! emitted by any of them fails **this crate's** compile, because nothing
//! third-party sits in its extern prelude. Same spirit as the trybuild
//! diagnostics suites — macro hygiene is proven by compiling a consumer, not
//! by reading emissions.
//!
//! Extend this crate whenever a decorator is added. Decorators excluded by
//! the documented contract (see `framework.md`) are deliberately not
//! exercised: emitted derives, the entity-site trio
//! `::sea_orm`/`::uuid`/`::chrono`, and the HTTP handler surface —
//! `#[routes]`/`#[crud]` wrap each verb in poem's own `#[handler]`, whose
//! expansion targets the call-site prelude, so a controller crate owns `poem`
//! (and `nest-rs-interceptors`) by contract.
//!
//! **Not witnessed here:** `#[expose]` (`nest-rs-resource`). Exercising it
//! would require the entity-site trio + emitted derives, which reintroduces
//! third-party deps and defeats the zero-dep design — so its re-export
//! routing (`::nest_rs_resource::{async_trait, tracing, serde_json}`) rests on
//! review, not this compile proof.

pub mod gateway;
pub mod lifecycle;
pub mod listener;
pub mod module;
pub mod tasks;
