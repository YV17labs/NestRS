//! `#[indicators]` — the health probe registry's impl-half decorator.

use nest_rs::core::injectable;
use nest_rs::health::indicators;

/// Minimal indicator host.
#[injectable]
pub struct HygieneIndicator;

#[indicators]
impl HygieneIndicator {
    /// A probe reports by returning: `Ok` is up, `Err` is down with the
    /// error's `Display` as the reason.
    #[readiness]
    async fn ready(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    /// The other two kinds share the expansion, so one of each proves the
    /// registry entry the decorator submits per method.
    #[liveness]
    async fn alive(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[startup]
    async fn started(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
}
