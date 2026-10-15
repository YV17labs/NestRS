//! The [`Transport`] trait and the contribution a module attaches to gain one —
//! the seam between the container and anything that accepts inbound requests
//! on the app's behalf.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::container::Container;

/// The longest [`Transport::stop_bound`] the boot accepts: one day, above every
/// first-party drain window, and short enough that the instant a stop ends at
/// never overflows.
pub const MAX_STOP_BOUND: Duration = Duration::from_secs(86_400);

/// Anything that accepts inbound requests on behalf of the app — an HTTP
/// server, a scheduler, a queue worker, ….
///
/// [`crate::App::run`] drives every transport through the same steps: each
/// one's `configure`, in registration order; the init hooks; each one's
/// `listen`; then, the app [`Ready`](crate::AppState::Ready), every `serve`
/// at once under the app's stop token, whose [`CancellationToken`] fires the
/// instant the app turns [`Draining`](crate::AppState::Draining) — at
/// `SIGINT` / `SIGTERM`, or when another transport fails. Nothing serves until
/// every transport listens, so a port taken or TLS material that cannot serve
/// fails the boot before any transport has run anything.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Scan the container for the surfaces this transport serves and wire them
    /// up, before any request is accepted.
    async fn configure(&mut self, container: &Container) -> Result<()>;
    /// Take what serving needs — bind a socket, check TLS material, subscribe —
    /// after the init hooks; the app turns [`Ready`](crate::AppState::Ready)
    /// once every transport has listened.
    ///
    /// A transport that takes something here takes it here, never in
    /// [`serve`](Self::serve): the default listens on nothing, and a transport
    /// keeping it while binding in `serve` counts as ready before it is.
    async fn listen(&mut self) -> Result<Listening> {
        Ok(Listening::default())
    }
    /// Accept requests until `cancel` fires, then shut down gracefully within
    /// [`stop_bound`](Self::stop_bound). Returning before `cancel` fires —
    /// `Ok` included — stops the app.
    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()>;
    /// The longest [`serve`](Self::serve) takes to return once `cancel` fires —
    /// its drain window, then
    /// [`SHUTDOWN_SETTLE_TIMEOUT`](crate::SHUTDOWN_SETTLE_TIMEOUT). Finite, and
    /// at most [`MAX_STOP_BOUND`]: the boot refuses more
    /// ([`UnboundedTransportError`](crate::UnboundedTransportError)), and
    /// [`App::run`](crate::App::run) abandons a transport still serving at its
    /// bound plus the settle.
    fn stop_bound(&self) -> Duration;
}

/// What a transport listens on, for the `app ready` line and the test harness.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Listening {
    /// Each socket address the transport bound, its real port once bound to `0`.
    pub addresses: Vec<SocketAddr>,
}

impl Listening {
    /// Listening on `addresses`.
    pub fn on(addresses: impl IntoIterator<Item = SocketAddr>) -> Self {
        Self {
            addresses: addresses.into_iter().collect(),
        }
    }
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
