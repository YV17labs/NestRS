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
//! exercised: emitted derives and the entity-site trio
//! `::sea_orm`/`::uuid`/`::chrono`, whose expansions target the call-site
//! prelude because the developer's own source writes them.
//!
//! `#[controller]`/`#[routes]` **are** witnessed ([`controller`]).
//! `#[routes]` emits its own `Endpoint` impl instead of wrapping poem's
//! `#[handler]`, so nothing in the expansion resolves against the call-site
//! prelude and a controller crate needs no `poem` line — the exclusion this
//! paragraph used to record no longer describes the macro.
//!
//! **Not witnessed here:** `#[expose]` (`nest-rs-resource`). Exercising it
//! would require the entity-site trio + emitted derives, which reintroduces
//! third-party deps and defeats the zero-dep design — so its re-export
//! routing (`::nest_rs_resource::{async_trait, tracing, serde_json}`) rests on
//! review, not this compile proof.

pub mod controller;
pub mod gateway;
pub mod lifecycle;
pub mod listener;
pub mod module;
pub mod tasks;
pub mod tool;
