//! Booted app with no HTTP client, for queue workers and schedulers.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Result;
use nest_rs_core::{App, AppLifecycle, Container, Transport};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Runs non-HTTP [`Transport`]s on a cancellable background task so a test
/// can enqueue, observe, then shut the transport down.
///
/// [`close`](Self::close) ends it as [`App::close`] does, its transports
/// stopped first; dropping it instead runs no shutdown hook and closes nothing.
pub struct HeadlessApp {
    app: App,
    /// Every transport [`spawn_transport`](Self::spawn_transport) started.
    transports: TaskTracker,
    /// The longest [`Transport::stop_bound`] among them.
    stop_bound: Mutex<Duration>,
}

impl HeadlessApp {
    pub(crate) fn new(app: App) -> Self {
        Self {
            app,
            transports: TaskTracker::new(),
            stop_bound: Mutex::new(Duration::ZERO),
        }
    }

    /// The DI [`Container`], for resolving providers directly in assertions.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// The app's [`AppLifecycle`].
    pub fn lifecycle(&self) -> &Arc<AppLifecycle> {
        self.app.lifecycle()
    }

    /// Run the app's init phases — once, as [`App::init`] does. An init that
    /// fails first stops the transports [`spawn_transport`](Self::spawn_transport)
    /// started, within the longest of their stop bounds, then runs the
    /// teardown [`App::init`] runs.
    pub async fn init(&self) -> Result<()> {
        let bound = *self
            .stop_bound
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        nest_rs_core::__private::init_tearing_down_after(
            &self.app,
            stop_within(&self.transports, bound),
        )
        .await
    }

    /// Configure and run a non-HTTP [`Transport`] on a cancellable background
    /// task, returning a [`TransportHandle`] to observe then shut it down. It
    /// serves under the app's stop token, so [`close`](Self::close) stops it;
    /// a `serve` that fails is filed at `error` with its chain.
    pub async fn spawn_transport<T: Transport>(&self, mut transport: T) -> Result<TransportHandle> {
        self.configure_transport(&mut transport).await?;
        Ok(self.serve_transport(transport))
    }

    /// Configure `transport` against the container, refusing what a site it
    /// composed refused, as [`App::run`] does.
    pub(crate) async fn configure_transport<T: Transport>(&self, transport: &mut T) -> Result<()> {
        transport.configure(self.container()).await?;
        match nest_rs_core::__private::take_site_refusal(self.container()) {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        }
    }

    /// Serve a configured `transport` on the tracked background task.
    pub(crate) fn serve_transport<T: Transport>(&self, transport: T) -> TransportHandle {
        {
            let mut longest = self
                .stop_bound
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *longest = (*longest).max(transport.stop_bound());
        }
        let cancel = self.lifecycle().stopping();
        let token = cancel.clone();
        let join = self.transports.spawn(async move {
            let served = Box::new(transport).serve(token).await;
            // Its handle is often dropped unread: `close` awaits the tracker.
            if let Err(error) = &served {
                nest_rs_core::tracing::error!(
                    target: nest_rs_core::target::APP,
                    transport = std::any::type_name::<T>(),
                    error = %nest_rs_core::error_message(&**error),
                    "a transport the harness spawned failed",
                );
            }
            served
        });
        TransportHandle { cancel, join }
    }

    /// Stop the app as [`App::close`] does: it turns
    /// [`Draining`](nest_rs_core::AppState::Draining), the transports it spawned
    /// stop within the longest of their stop bounds — one still serving past it
    /// is abandoned at `warn` — then the shutdown phases run in reverse, the
    /// resources close, and the container drops.
    pub async fn close(self) {
        let HeadlessApp {
            app,
            transports,
            stop_bound,
        } = self;
        let bound = stop_bound
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        nest_rs_core::__private::close_after(app, stop_within(&transports, bound)).await;
    }
}

/// Wait for every tracked transport to return once the app's stop token fired,
/// within `bound`; one still serving past it is abandoned at `warn`.
async fn stop_within(transports: &TaskTracker, bound: Duration) {
    transports.close();
    if tokio::time::timeout(bound, transports.wait())
        .await
        .is_err()
    {
        nest_rs_core::tracing::warn!(
            target: nest_rs_core::target::APP,
            stop_bound_ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
            "a transport the harness spawned was still serving past its stop bound: abandoned",
        );
    }
}

/// Dropping detaches the task; [`shutdown`](Self::shutdown) cancels and awaits.
pub struct TransportHandle {
    cancel: CancellationToken,
    join: JoinHandle<Result<()>>,
}

impl TransportHandle {
    /// Cancel the transport task and await its exit, surfacing its error.
    pub async fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.join.await.map_err(|e| anyhow::anyhow!(e))?
    }
}
