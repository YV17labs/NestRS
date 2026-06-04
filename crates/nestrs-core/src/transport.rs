use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::container::Container;

/// Anything that accepts inbound requests on behalf of the app — an HTTP
/// server, MCP-over-stdio loop, gRPC server, ….
///
/// Lifecycle only: protocol-level concerns (message patterns, retries, ack
/// semantics) live in the transport's own crate.
///
/// [`crate::App::run`] awaits `configure` on each transport in registration
/// order (a transport scans its surfaces via
/// [`DiscoveryService`](crate::DiscoveryService) here), then spawns every
/// `serve` future with a shared [`CancellationToken`] that SIGTERM/SIGINT
/// triggers.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn configure(&mut self, container: &Container) -> Result<()>;
    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()>;
}

/// A transport contributed by a module — the only way an app gains one.
/// Drained by [`App::run`](crate::App::run) at boot.
///
/// Modules attach one with
/// [`ContainerBuilder::provide_meta`](crate::ContainerBuilder::provide_meta):
///
/// ```ignore
/// impl Module for ScheduleModule {
///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
///         builder.provide_meta(TransportContribution {
///             name: "Scheduler",
///             build: |_| Ok(Box::new(Scheduler::new())),
///         })
///     }
/// }
/// ```
///
/// A module that is not imported never runs its `register`, so its
/// contribution never lands in the container — module-gating is free.
pub struct TransportContribution {
    /// Human-readable label used in boot logs.
    pub name: &'static str,
    /// Build the transport at boot. Sees the assembled container, so the
    /// transport may resolve providers eagerly if it needs to.
    pub build: fn(&Container) -> Result<Box<dyn Transport>>,
}
