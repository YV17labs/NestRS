//! **Port** templates — a transport-agnostic feature slice (`g feature`).
//!
//! The bare port: a `mod.rs` index, a `module.rs` DI module, and a
//! `service.rs` with a `count()` stand-in. Add a transport with
//! `g http|graphql|ws|queue|schedule|mcp <feature>`; each adapter delegates
//! to this service.

pub const MOD: &str = r#"mod module;
mod service;

pub use module::{{module}};
pub use service::{{service}};
"#;

pub const MODULE: &str = r#"use nest_rs_core::module;

use super::service::{{service}};

#[module(providers = [{{service}}])]
pub struct {{module}};
"#;

pub const SERVICE: &str = r#"use nest_rs_core::injectable;

#[injectable]
#[derive(Default)]
pub struct {{service}};

impl {{service}} {
    pub fn count(&self) -> usize {
        0
    }
}
"#;
