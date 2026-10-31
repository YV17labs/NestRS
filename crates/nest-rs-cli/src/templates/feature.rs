//! **Port** templates — a transport-agnostic feature slice (`g feature`).
//!
//! The bare port: a `mod.rs` index, a `module.rs` DI module, and a
//! `service.rs` with a `count()` stand-in. It emits nothing, so it declares no
//! log target: the generator whose output logs adds one ([`TARGET`]). Add a transport with
//! `g http|graphql|ws|queue|schedule|mcp|events <feature>`; each adapter delegates
//! to this service.

pub(crate) const MOD: &str = r#"mod module;
mod service;

pub use module::{{module}};
pub use service::{{service}};
"#;

pub(crate) const MODULE: &str = r#"use nest_rs::core::module;

use super::service::{{service}};

#[module(providers = [{{service}}])]
pub struct {{module}};
"#;

/// The port's `#[subject]`, written by the first adapter over a port with no
/// entity that gates an operation on it.
pub(crate) const SUBJECT: &str = r#"use nest_rs::authz::subject;

/// What an operation over this port is gated on until the port serves rows: a
/// capability no table holds.
#[subject]
pub struct {{entity}};
"#;

pub(crate) const SERVICE: &str = r#"use nest_rs::core::injectable;

#[injectable]
pub struct {{service}};

impl {{service}} {
    pub fn count(&self) -> usize {
        0
    }
}
"#;

/// The feature's log target, which every event it emits names: declared at the
/// top of its `mod.rs` by the first generator whose output logs.
pub(crate) const TARGET: &str = r#"pub const TARGET: &str = "features::{{snake}}";
"#;
