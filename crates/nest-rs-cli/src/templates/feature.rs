//! **Port** templates — a transport-agnostic feature slice (`g feature`).
//!
//! The bare port: a `mod.rs` index that also declares the feature's log
//! target, a `module.rs` DI module, and a `service.rs` with a `count()`
//! stand-in. Add a transport with
//! `g http|graphql|ws|queue|schedule|mcp|events <feature>`; each adapter delegates
//! to this service.

pub(crate) const MOD: &str = r#"pub const TARGET: &str = "features::{{snake}}";

mod module;
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
