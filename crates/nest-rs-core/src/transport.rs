//! The [`Transport`] trait and the [`TransportContribution`] a module attaches
//! to gain one — the seam between the container and anything that accepts
//! inbound requests on the app's behalf.

use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::container::Container;

/// Anything that accepts inbound requests on behalf of the app — an HTTP
/// server, a scheduler, a queue worker, gRPC server, ….
///
/// Lifecycle only: protocol-level concerns (message patterns, retries, ack
/// semantics) live in the transport's own crate.
///
/// [`crate::App::run`] awaits `configure` on each transport in registration
/// order (a transport scans its surfaces via
/// [`Discovery`](crate::Discovery) here), then spawns every
/// `serve` future with a shared [`CancellationToken`] that SIGTERM/SIGINT
/// triggers.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Scan the container for the surfaces this transport serves and wire them
    /// up, before any request is accepted. Runs at boot in registration order.
    async fn configure(&mut self, container: &Container) -> Result<()>;
    /// Accept requests until `cancel` fires, then shut down gracefully — and
    /// return within a bound of the transport's own, since
    /// [`App::run`](crate::App::run) awaits every `serve` before the shutdown
    /// hooks run and holds none of its own: a transport that waits on work that
    /// never ends holds the process until the orchestrator kills it. The shape
    /// every framework transport keeps is a window for what it still runs, then
    /// [`SHUTDOWN_SETTLE_TIMEOUT`](crate::SHUTDOWN_SETTLE_TIMEOUT) for what it
    /// stopped to unwind, and [`stop_bound`](Self::stop_bound) states it.
    /// Spawned after every transport has been configured.
    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()>;
    /// The longest [`serve`](Self::serve) takes to return once `cancel` fires,
    /// at the configuration [`configure`](Self::configure) left — the window
    /// for what it still runs, then the settle for what it stopped.
    ///
    /// Required, with no default, because the way down is a sum the grace
    /// period has to hold: [`App::run`](crate::App::run) files the longest
    /// bound of the transports it mounted beside the shutdown hooks' budget on
    /// its boot line, and `nest-rs-testing` sums every framework transport's
    /// default bound under a Kubernetes pod's default grace. A transport whose
    /// stop is unbounded says [`Duration::MAX`](std::time::Duration::MAX).
    fn stop_bound(&self) -> std::time::Duration;
}

/// A transport contributed by a module — the only way an app gains one.
/// Drained by [`App::run`](crate::App::run) at boot.
///
/// Modules attach one with
/// [`ContainerBuilder::provide_meta`](crate::ContainerBuilder::provide_meta):
///
/// ```
/// # use std::time::Duration;
/// # use nest_rs_core::{Container, ContainerBuilder, Discovery, Imported, Module, Transport, TransportContribution, async_trait};
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
///     fn register(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
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
///
/// A module that is not imported never runs its `register`, so its
/// contribution never lands in the container — module-gating is free.
///
/// **Internal ABI** — macro/module-constructed, lockstep with `nest-rs-core`;
/// do not hand-construct.
#[doc(hidden)]
pub struct TransportContribution {
    /// Human-readable label used in boot logs.
    pub name: &'static str,
    /// Build the transport at boot. Sees the assembled container, so the
    /// transport may resolve providers eagerly if it needs to.
    pub build: fn(&Container) -> Result<Box<dyn Transport>>,
}
