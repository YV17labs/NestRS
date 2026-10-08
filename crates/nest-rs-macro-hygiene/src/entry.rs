//! `#[nest_rs::main]` builds its tokio runtime through `::nest_rs::core` alone.
//! Not named `main.rs`, which Cargo would build as a binary.

use nest_rs::core::App;

/// The body's `?` converts into the declared return, as in an `async fn main`.
#[nest_rs::main]
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    App::new::<crate::module::MacroHygieneModule>()?;
    Ok(())
}
