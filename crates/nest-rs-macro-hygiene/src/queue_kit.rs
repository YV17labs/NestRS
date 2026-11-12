//! `queue_kit!`, reached through the umbrella's `queue` and `testing` features
//! alone, as a queue driver's suite reaches it: `just lint` compiles it without
//! `http` too.

use std::time::Duration;

use nest_rs::core::anyhow;
use nest_rs::queue::QueueName;
use nest_rs::testing::queue::{KitBackend, KitLease};
use nest_rs::testing::{HeadlessApp, TestAppBuilder};

/// A backend the kit is written against, as a driver declares its own.
pub struct HygieneQueueKit;

impl KitBackend for HygieneQueueKit {
    fn app(&self, _lease: KitLease) -> TestAppBuilder {
        HeadlessApp::builder()
    }

    fn lease(&self, lease: KitLease) -> Duration {
        match lease {
            KitLease::Brief => Duration::from_secs(3),
            KitLease::Lasting => Duration::from_secs(30),
        }
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
