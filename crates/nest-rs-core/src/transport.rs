//! The [`Transport`] trait and the contribution a module attaches to gain one —
//! the seam between the container and anything that accepts inbound requests
//! on the app's behalf.

use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::container::Container;

/// Anything that accepts inbound requests on behalf of the app — an HTTP
/// server, a scheduler, a queue worker, ….
///
/// [`crate::App::run`] awaits `configure` on each transport in registration
/// order, then spawns every `serve` future with a shared [`CancellationToken`]
/// that SIGTERM/SIGINT triggers.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Scan the container for the surfaces this transport serves and wire them
    /// up, before any request is accepted.
    async fn configure(&mut self, container: &Container) -> Result<()>;
    /// Accept requests until `cancel` fires, then shut down gracefully within
    /// [`stop_bound`](Self::stop_bound): [`App::run`](crate::App::run) awaits
    /// every `serve` with no bound of its own.
    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()>;
    /// The longest [`serve`](Self::serve) takes to return once `cancel` fires —
    /// its drain window, then
    /// [`SHUTDOWN_SETTLE_TIMEOUT`](crate::SHUTDOWN_SETTLE_TIMEOUT). A transport
    /// whose stop is unbounded says [`Duration::MAX`](std::time::Duration::MAX).
    fn stop_bound(&self) -> std::time::Duration;
}

pub(crate) use self::__private::TransportContribution;

/// `transport` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use anyhow::Result;

    use super::Transport;
    use crate::container::Container;

    /// A transport contributed by a module — the only way an app gains one.
    /// Drained by [`App::run`](crate::App::run) at boot.
    ///
    /// Modules attach one with
    /// [`ContainerBuilder::provide_meta`](crate::ContainerBuilder::provide_meta):
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use nest_rs_core::{Container, ContainerBuilder, Discovery, Registering, Module, Transport, async_trait};
    /// # use nest_rs_core::__private::TransportContribution;
    /// # struct ScheduleModule;
    /// # struct Scheduler;
    /// # impl Scheduler {
    /// #     fn new() -> Self { Self }
    /// # }
    /// # #[async_trait]
    /// # impl Transport for Scheduler {
    /// #     async fn configure(&mut self, _: &Container) -> anyhow::Result<()> { Ok(()) }
    /// #     async fn serve(self: Box<Self>, cancel: tokio_util::sync::CancellationToken) -> anyhow::Result<()> { cancel.cancelled().await; Ok(()) }
    /// #     fn stop_bound(&self) -> Duration { Duration::ZERO }
    /// # }
    /// impl Module for ScheduleModule {
    ///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
    ///         builder.provide_meta(TransportContribution {
    ///             name: "Scheduler",
    ///             build: |_| Ok(Box::new(Scheduler::new())),
    ///         })
    ///     }
    /// }
    /// # let container = Container::builder().import::<ScheduleModule>().build();
    /// # let contributed = Discovery::new(&container).meta::<TransportContribution>();
    /// # assert_eq!(contributed.iter().map(|c| c.meta.name).collect::<Vec<_>>(), ["Scheduler"]);
    /// ```
    pub struct TransportContribution {
        /// Human-readable label used in boot logs.
        pub name: &'static str,
        /// Build the transport at boot, from the assembled container.
        pub build: fn(&Container) -> Result<Box<dyn Transport>>,
    }
}
