//! [`KitBackend`] — what a queue backend supplies the kit.

use std::future::Future;
use std::time::Duration;

use nest_rs_queue::QueueName;

use crate::TestAppBuilder;

/// What a queue backend supplies the kit: an app reaching it, its lease, and
/// the hooks a case needs on its storage.
pub trait KitBackend: Send + Sync + 'static {
    /// Whether the backend lives in this process, so each case runs on paused
    /// time. Such a backend keeps its leases and delays on tokio's clock: on
    /// `std::time` they would never lapse.
    const IN_PROCESS: bool = false;

    /// A test app reaching this backend — its connection, and the binding of
    /// its producer and consumer — over a store no other test of the run
    /// reaches, leasing deliveries for [`lease`](Self::lease). Every call builds
    /// a consumer of its own, as each replica has.
    fn app(&self) -> TestAppBuilder;

    /// How long a delivery's lease lasts in [`app`](Self::app): short, since a
    /// case waits some out.
    fn lease(&self) -> Duration;

    /// Remove everything the backend keeps for `queue`, so a case starts from
    /// nothing.
    fn purge(&self, queue: &QueueName) -> impl Future<Output = anyhow::Result<()>> + Send;

    /// Make every lease held on `queue` lapse now, its holder and delivery
    /// count unchanged — what a holder that stopped renewing leaves — so the
    /// next receive of any worker reclaims the job.
    fn lapse(&self, queue: &QueueName) -> impl Future<Output = anyhow::Result<()>> + Send;

    /// Hand every delivery held on `queue` to another holder that lapses at
    /// once — what a holder frozen past its lease meets: its lease is taken,
    /// and the next receive of any worker reclaims the job.
    fn take(&self, queue: &QueueName) -> impl Future<Output = anyhow::Result<()>> + Send;
}
