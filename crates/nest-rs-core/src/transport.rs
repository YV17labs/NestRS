//! The [`Transport`] trait and the record a module attaches to gain one — the
//! seam between the container and anything that accepts inbound requests on
//! the app's behalf.

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
/// A module attaches one with
/// [`ContainerBuilder::provide_transport`](crate::ContainerBuilder::provide_transport),
/// and [`crate::App::run`] drives every transport through the same steps: each
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

/// What a second transport of one type is refused with.
pub(crate) const ONE_TRANSPORT_PER_TYPE: &str = "An app runs one transport of each type: import \
     the module that attaches it by type, bare beside the one `for_root` that configures it.";

/// What builds the transport from the assembled container, its type erased.
type Build = Box<dyn Fn(&Container) -> Result<Box<dyn Transport>> + Send + Sync>;

/// A transport a module attached with
/// [`ContainerBuilder::provide_transport`](crate::ContainerBuilder::provide_transport)
/// — readable through
/// [`Discovery::meta`](crate::Discovery::meta), built only by the kernel; its
/// name is the transport's short type name.
///
/// ```compile_fail
/// use nest_rs_core::TransportContribution;
///
/// let _ = TransportContribution {
///     name: "Scheduler".to_owned(),
///     build: Box::new(|_| unreachable!()),
/// };
/// ```
pub struct TransportContribution {
    name: String,
    build: Build,
}

impl TransportContribution {
    pub(crate) fn new<T: Transport>(build: fn(&Container) -> Result<T>) -> Self {
        Self {
            name: crate::type_name::short_type_name::<T>(),
            build: Box::new(move |container| Ok(Box::new(build(container)?))),
        }
    }

    /// The transport, as the boot lines name it (`"HttpTransport"`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Build the transport from the assembled `container`.
    pub(crate) fn build(&self, container: &Container) -> Result<Box<dyn Transport>> {
        (self.build)(container)
    }
}
