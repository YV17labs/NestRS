//! `#[injectable]` + `#[hooks]` — including the run-fn signature the macro
//! emits (`::nest_rs::core::anyhow::Result`), the M1 regression: a `#[hooks]`
//! consumer without a direct `anyhow` dependency must compile.

use nest_rs::core::{hooks, injectable};

/// Lifecycle host with no dependencies — the minimal `#[hooks]` consumer.
#[injectable]
pub struct HygieneLifecycle;

#[hooks]
impl HygieneLifecycle {
    /// Bare (infallible) form — the macro adapts the `()` return to `Ok(())`.
    #[on_application_bootstrap]
    async fn boot(&self) {}

    /// Fallible form — the error converts `Into` the emitted
    /// `::nest_rs::core::anyhow::Result` without `anyhow` in this crate.
    #[on_module_destroy]
    async fn shutdown(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    /// A synchronous hook is called without an `.await`.
    #[on_module_init]
    fn init(&self) {}

    /// A hook compiled out takes its registration with it — the missing item it
    /// names is never looked up.
    #[on_application_shutdown]
    async fn steady(&self) -> Result<(), crate::never::Never> {
        Ok(())
    }

    #[cfg(any())]
    #[on_application_shutdown]
    async fn compiled_out(&self) -> crate::does_not_exist::Error {
        crate::does_not_exist::run()
    }
}
