//! `#[injectable]` + `#[hooks]`: a consumer without a direct `anyhow`
//! dependency compiles the emitted `::nest_rs::core::anyhow::Result`.

use nest_rs::core::{hooks, injectable};

#[injectable]
pub struct HygieneLifecycle;

#[hooks]
impl HygieneLifecycle {
    #[on_application_bootstrap]
    async fn boot(&self) {}

    #[on_module_destroy]
    async fn shutdown(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[on_module_init]
    fn init(&self) {}

    #[on_application_shutdown]
    async fn steady(&self) -> Result<(), crate::never::Never> {
        Ok(())
    }

    /// A hook compiled out takes its registration with it.
    #[cfg(any())]
    #[on_application_shutdown]
    async fn compiled_out(&self) -> crate::does_not_exist::Error {
        crate::does_not_exist::run()
    }
}
