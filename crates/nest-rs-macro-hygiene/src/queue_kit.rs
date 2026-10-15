//! `queue_kit!` and `TestAuthority`, reached through the umbrella's `queue`
//! and `testing` features alone, as a queue driver's suite reaches them.

use std::time::Duration;

use nest_rs::core::anyhow;
use nest_rs::queue::QueueName;
use nest_rs::testing::queue::KitBackend;
use nest_rs::testing::{TestApp, TestAppBuilder, TestAuthority};

/// A backend the kit is written against, as a driver declares its own.
pub struct HygieneQueueKit;

impl KitBackend for HygieneQueueKit {
    fn app(&self) -> TestAppBuilder {
        TestApp::builder()
    }

    fn lease(&self) -> Duration {
        Duration::from_secs(1)
    }

    async fn purge(&self, _queue: &QueueName) -> anyhow::Result<()> {
        Ok(())
    }

    async fn lapse(&self, _queue: &QueueName) -> anyhow::Result<()> {
        Ok(())
    }

    async fn take(&self, _queue: &QueueName) -> anyhow::Result<()> {
        Ok(())
    }
}

// Outside a test build each case is dropped with its `#[test]`; inside one it
// would run against a backend that holds nothing, so the invocation stays out.
#[cfg(not(test))]
nest_rs::testing::queue_kit!(HygieneQueueKit);

/// The authority a TLS test double presents, under `testing` alone.
pub fn hygiene_authority() -> TestAuthority {
    TestAuthority::new()
}
