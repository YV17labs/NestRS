//! Compile-time witness of macro path hygiene (`macros.md`).
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
//! **Each witness compiles under its capability's feature alone** — the
//! crate's features mirror the umbrella's, one per capability that owns a
//! decorator — so the union proves no decorator needs a second manifest line,
//! and each feature on its own proves its capability pulls everything its
//! decorators emit. The kernel's decorators (`#[module]`, `#[injectable]`,
//! `#[hooks]`, `#[nest_rs::main]`) are witnessed under no feature at all.
//!
//! Extend this crate whenever a decorator is added. Emitted derives are the
//! one class deliberately not exercised (see `macros.md`): a derive without a
//! `crate = ` override targets the call-site prelude by construction.
//!
//! `#[resolver]` **is** witnessed ([`resolver`]), and it is the case this file
//! most needed: it wraps async-graphql's own `#[Object]`, a third-party macro
//! that roots its expansion at whatever the *call site's* manifest declares.
//! 2.0.0 shipped with that fallback live, so the lead snippet of `/graphql/`
//! did not compile behind the documented install line; the `crate = ` override
//! that fixes it is invisible to review and visible here.
//!
//! `#[controller]`/`#[routes]` **are** witnessed ([`controller`]).
//! `#[routes]` emits its own `Endpoint` impl instead of wrapping poem's
//! `#[handler]`, so nothing in the expansion resolves against the call-site
//! prelude and a controller crate needs no `poem` line — the exclusion this
//! paragraph used to record no longer describes the macro.
//!
//! `#[expose]` and `#[crud]` **are** witnessed ([`entity`], [`crud`]), and so
//! is `#[authorize(Action, Entity)]` at all four edges, against that entity.
//! An entity can live here because sea-orm's derives emit *relative*
//! `sea_orm::` paths, which `use nest_rs::seaorm::sea_orm;` in the entity's
//! module satisfies — no `sea-orm` line. `#[crud]` is the case that proved the
//! witness must reach them: it emitted `::uuid::Uuid` for three routes, so a
//! controller whose source never wrote `uuid` failed with `E0433` blamed on the
//! attribute, and nothing compiled it behind one manifest line.
//!
//! **The limit:** a witness proves the arms it compiles and no others. An arm of
//! a decorator applied nowhere here — a key no witness writes, a feature
//! combination the matrix does not build — is proved only where something else
//! compiles it, such as `nest-rs-cli`'s scaffold e2e; and that proves nothing
//! when the generated project happens to declare the crate the arm names.
//!
//! [`canary`] is the witness's second mandate: one `#[expect]` per entry of the
//! repository's `clippy.toml`, so an entry that stops resolving fails the build
//! instead of switching its rule off in silence.
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_macros,
        reason = "canary: proves clippy.toml's tokio::main entry resolves (see `canary`)"
    )
)]

pub mod canary;
#[cfg(feature = "config")]
pub mod config;
#[cfg(feature = "http")]
pub mod controller;
#[cfg(all(feature = "http", feature = "seaorm"))]
pub mod crud;
#[cfg(feature = "graphql")]
pub mod dataloader;
#[cfg(feature = "seaorm")]
pub mod entity;
pub mod entry;
#[cfg(feature = "ws")]
pub mod gateway;
#[cfg(feature = "health")]
pub mod indicators;
#[cfg(feature = "http")]
pub mod interceptor;
pub mod lifecycle;
#[cfg(feature = "events")]
pub mod listener;
pub mod module;
pub mod prelude;
#[cfg(feature = "queue")]
pub mod processor;
#[cfg(feature = "graphql")]
pub mod resolver;
#[cfg(feature = "schedule")]
pub mod tasks;
#[cfg(feature = "mcp")]
pub mod tool;
#[cfg(feature = "seaorm")]
pub mod wire_enum;
