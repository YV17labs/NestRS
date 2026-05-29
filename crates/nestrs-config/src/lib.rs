//! Configuration loading for nestrs — the `@nestjs/config` analog.
//!
//! Kept out of `nestrs-core` so the kernel (container, modules, lifecycle) does
//! not drag `figment` into every crate that depends on it; an app or framework
//! crate that needs configuration depends on `nestrs-config` directly.

mod error;
mod loader;

pub use error::{ConfigError, Result};
pub use loader::{env_var, load, load_validated};
