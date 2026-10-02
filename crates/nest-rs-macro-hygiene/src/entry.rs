//! `#[nest_rs::main]` — at the umbrella's root, as an app writes it. Its
//! expansion builds a tokio runtime, so this is the witness that it does so
//! through `::nest_rs::core` alone: this crate declares no `tokio`, and an app's
//! manifest needs none for its `main`. Not named `main.rs`, which Cargo would
//! build as a binary.

use nest_rs::core::App;

/// An entry point written the way the scaffold writes one, with the body's `?`
/// converting into the declared return as it would in an `async fn main`.
#[nest_rs::main]
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    App::new::<crate::module::MacroHygieneModule>()?;
    Ok(())
}
