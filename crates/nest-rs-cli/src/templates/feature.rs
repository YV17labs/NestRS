//! **Port** templates — a transport-agnostic feature slice (`g feature`).
//!
//! The bare port: a `mod.rs` index, a `module.rs` DI module, and a
//! `service.rs` with a `count()` stand-in. Add a transport with
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

pub(crate) const SERVICE: &str = r#"use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
pub struct {{service}};

impl {{service}} {
    pub fn count(&self) -> usize {
        0
    }
}
"#;
