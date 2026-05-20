//! Core building blocks of the nestrs framework.

pub mod config;
pub mod container;
pub mod error;
pub mod logging;
pub mod module;

pub use container::{Container, ContainerBuilder};
pub use error::{Error, Result};
pub use module::Module;

pub use nestrs_macros::{controller, injectable, module, routes};
