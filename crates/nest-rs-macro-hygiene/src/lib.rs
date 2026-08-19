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
//! `#[wire_enum]` **is** witnessed ([`wire_enum`]) even though its sibling
//! `#[expose]` is not: an enum is a plain Rust item, so nothing in its
//! expansion needs an entity, and the four derives it routes (`Serialize`,
//! `Deserialize`, `JsonSchema`, `async_graphql::Enum`) are exactly the class a
//! zero-dep manifest can decide.
//!
//! **Not witnessed here:** `#[expose]` and `#[crud]` (`nest-rs-resource`).
//! Exercising them needs a real entity, and an entity cannot live in a zero-dep
//! crate: `DeriveEntityModel` roots its own expansion at the call site's
//! `sea_orm` and — checked against sea-orm-macros 2.0, not assumed — offers no
//! `crate = ` override to redirect it. That is the entity-site exception, and it
//! is why the exclusion is principled rather than merely convenient.
//!
//! It is also why the two are excluded for *different* reasons, which matters:
//! `#[expose]` sits on an entity, whose own source legitimately writes
//! `sea_orm`; `#[crud]` sits on a **controller**, whose source writes nothing
//! but `std`, `nest_rs` and `crate::`. Only the first has an excuse. `#[crud]`'s
//! contract is therefore proved in `nest-rs-cli`'s e2e by
//! `crud_needs_no_dependency_the_controller_does_not_name`, which builds the one
//! tree that can observe it: a resource whose crate declares no `uuid`.
//!
//! Their path rooting is not left to review, though — `tests/integration/`
//! carries the static half of this witness: it reads every `*-macros` source
//! and fails on a path rooted outside the framework, whichever decorator emits
//! it. That scan is what a compile proof cannot be, namely exhaustive over
//! decorators no zero-dep consumer can call; the compile proof is what a scan
//! cannot be, namely decisive about what actually resolves. Neither replaces
//! the other, and `#[crud]` is the case that proved it: it emitted
//! `::uuid::Uuid` for three routes and one resolver argument, so a controller
//! whose own source never wrote `uuid` failed with `E0433` blamed on the
//! attribute — invisible here, and invisible to the generator's e2e too, which
//! adds `uuid` for an unrelated reason.

pub mod config;
pub mod controller;
pub mod dataloader;
pub mod gateway;
pub mod indicators;
pub mod interceptor;
pub mod lifecycle;
pub mod listener;
pub mod module;
pub mod prelude;
pub mod processor;
pub mod resolver;
pub mod tasks;
pub mod tool;
pub mod wire_enum;
