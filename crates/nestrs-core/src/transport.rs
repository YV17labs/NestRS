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
