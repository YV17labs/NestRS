//! Compile-time witness of macro path hygiene.
//!
//! This crate depends **only** on `nest-rs-*` surface crates, so a bare
//! third-party path (`::anyhow`, `::tracing`, …) emitted by any decorator used
//! here fails this crate's compile; its suite pins each decorator's refusals
//! through the umbrella, as a developer meets them.
//!
//! Each witness compiles under its capability's feature alone, mirroring the
//! umbrella's features; the kernel's decorators compile under none. Extend this
//! crate whenever a decorator is added. Emitted derives without a `crate = `
//! override are not exercised: they target the call-site prelude by construction.
//!
//! **The limit:** a witness proves the arms it compiles and no others — a key
//! no witness writes, or a feature combination the matrix does not build, is
//! unproved here.

#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

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
pub mod never;
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
