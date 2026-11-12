//! `#[indicators]` — the health probe registry's impl-half decorator.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

#[injectable]
pub struct HygieneIndicator;

#[indicators]
impl HygieneIndicator {
    #[readiness]
    async fn ready(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[liveness]
    async fn alive(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[startup]
    fn started(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[readiness]
    async fn steady(&self) -> Result<(), crate::never::Never> {
        Ok(())
    }

    /// A probe compiled out takes its registry entry with it.
    #[cfg(any())]
    #[readiness]
    async fn compiled_out(&self) -> crate::does_not_exist::Answer {
        crate::does_not_exist::probe()
    }
}
