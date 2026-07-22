//! `#[injectable]` + `#[hooks]` — including the run-fn signature the macro
//! emits (`::nest_rs_core::anyhow::Result`), the M1 regression: a `#[hooks]`
//! consumer without a direct `anyhow` dependency must compile.

use nest_rs_core::{hooks, injectable};

/// Lifecycle host with no dependencies — the minimal `#[hooks]` consumer.
#[injectable]
pub struct HygieneLifecycle;

#[hooks]
impl HygieneLifecycle {
    /// Bare (infallible) form — the macro adapts the `()` return to `Ok(())`.
    #[on_application_bootstrap]
    async fn boot(&self) {}

    /// Fallible form — the error converts `Into` the emitted
    /// `::nest_rs_core::anyhow::Result` without `anyhow` in this crate.
    #[on_module_destroy]
    async fn shutdown(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}
