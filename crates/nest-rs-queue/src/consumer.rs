//! Consumer seam.
//!
//! A backend's [`JobConsumer`] receives the boot-time list of
//! [`ProcessMethod`]s reachable in this app and runs until the shutdown
//! signal fires. Filtering by
//! [`ReachableProviders`](::nest_rs_core::ReachableProviders) happens **before**
//! `run` — the backend just dispatches.

use async_trait::async_trait;
use nest_rs_core::Container;
use tokio_util::sync::CancellationToken;

use crate::method::ProcessMethod;

/// Drains a list of `#[process]` methods until cancellation. One per app —
/// the `Transport` a queue backend contributes typically wraps a
/// [`JobConsumer`] and forwards the cancellation token.
#[async_trait]
pub trait JobConsumer: Send + Sync + 'static {
    /// Run the consumer loop. `methods` is the access-graph-filtered set of
    /// process methods this backend is responsible for; `container` resolves
    /// the providers each handler dispatches into; `cancel` triggers
    /// graceful shutdown.
    async fn run(
        self: Box<Self>,
        methods: Vec<&'static ProcessMethod>,
        container: Container,
        cancel: CancellationToken,
    ) -> anyhow::Result<()>;
}
