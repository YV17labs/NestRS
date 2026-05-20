use async_trait::async_trait;
use nestrs_core::injectable;

/// Pluggable probe contract. Apps can substitute their own implementation by
/// binding it via `#[module(providers = [MyService as dyn HealthCheck])]`,
/// which replaces the default registered by [`crate::HealthModule`].
#[async_trait]
pub trait HealthCheck: Send + Sync + 'static {
    async fn is_live(&self) -> bool {
        true
    }

    async fn is_ready(&self) -> bool {
        true
    }

    async fn is_started(&self) -> bool {
        true
    }
}

#[injectable]
#[derive(Default)]
pub struct HealthService;

impl HealthCheck for HealthService {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_service_reports_live_ready_and_started() {
        let svc = HealthService;
        assert!(svc.is_live().await);
        assert!(svc.is_ready().await);
        assert!(svc.is_started().await);
    }
}
