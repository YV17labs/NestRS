//! Booted app with no transport of its own, for queue workers and schedulers.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Result, anyhow};
use nest_rs_core::{App, AppLifecycle, Container, Transport};
use tokio::task::{AbortHandle, Id, JoinHandle};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::TestAppBuilder;

/// Runs non-HTTP [`Transport`]s on a cancellable background task so a test
/// can enqueue, observe, then shut the transport down.
///
/// [`close`](Self::close) ends it as [`App::close`] does, its transports
/// stopped first; dropping it instead runs no shutdown hook and closes nothing.
pub struct HeadlessApp {
    app: App,
    /// Every transport [`spawn_transport`](Self::spawn_transport) started.
    transports: TaskTracker,
    /// Each of them still serving, with its [`Transport::stop_bound`].
    serving: Serving,
}

/// The transports still serving, in the order they were spawned.
type Serving = Arc<Mutex<Vec<Spawned>>>;

/// A transport still serving: its task, its name and stop bound, and the
/// handle that aborts it once abandoned.
struct Spawned {
    task: Id,
    name: &'static str,
    stop_bound: Duration,
    abort: AbortHandle,
}

impl HeadlessApp {
    /// Start a [`TestAppBuilder`] to register modules and override providers,
    /// then [`build_headless`](TestAppBuilder::build_headless): the entry of a
    /// suite that serves no HTTP, which compiles without the `http` feature.
    ///
    /// ```
    /// # use nest_rs_core::{injectable, module};
    /// use nest_rs_testing::HeadlessApp;
    ///
    /// #[injectable]
    /// struct Transcoder;
    ///
    /// #[module(providers = [Transcoder])]
    /// struct WorkerModule;
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let app = HeadlessApp::builder().module::<WorkerModule>().build_headless().await?;
    /// assert!(app.container().get::<Transcoder>().is_some());
    /// app.close().await;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder::new()
    }

    pub(crate) fn new(app: App) -> Self {
        Self {
            app,
            transports: TaskTracker::new(),
            serving: Serving::default(),
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
        nest_rs_core::__private::init_tearing_down_after(
            &self.app,
            stop_within(&self.transports, &self.serving),
        )
        .await
    }

    /// Configure a [`Transport`], have it [`listen`](Transport::listen), then
    /// run it on a cancellable background task, returning a
    /// [`TransportHandle`] to observe then shut it down: once this returns,
    /// what it listens on accepts. It serves under the app's stop token, so
    /// [`close`](Self::close) stops it; a `serve` that fails is filed at
    /// `error` with its chain, and one that panics is contained and filed at
    /// `error` once, as [`App::run`] files it.
    pub async fn spawn_transport<T: Transport>(&self, mut transport: T) -> Result<TransportHandle> {
        self.configure_transport(&mut transport).await?;
        transport.listen().await?;
        Ok(self.serve_transport(transport))
    }

    /// Configure `transport` against the container, refusing what a site it
    /// composed refused and a stop it cannot bound, as [`App::run`] does.
    pub(crate) async fn configure_transport<T: Transport>(&self, transport: &mut T) -> Result<()> {
        transport.configure(self.container()).await?;
        if let Some(refusal) = nest_rs_core::__private::take_site_refusal(self.container()) {
            return Err(refusal);
        }
        nest_rs_core::__private::check_stop_bound(
            std::any::type_name::<T>(),
            transport.stop_bound(),
        )?;
        Ok(())
    }

    /// Serve a configured, listening `transport` on the tracked background task.
    pub(crate) fn serve_transport<T: Transport>(&self, transport: T) -> TransportHandle {
        let name = std::any::type_name::<T>();
        let stop_bound = transport.stop_bound();
        let serving = Arc::clone(&self.serving);
        let cancel = self.lifecycle().stopping();
        let token = cancel.clone();
        // Held across the spawn, so the task cannot leave the list before it joins it.
        let mut still = locked(&self.serving);
        let join = self.transports.spawn(async move {
            let served = nest_rs_core::panic::contain(Box::new(transport).serve(token)).await;
            let task = tokio::task::id();
            locked(&serving).retain(|spawned| spawned.task != task);
            // Its handle is often dropped unread: `close` awaits the tracker.
            match served {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => {
                    nest_rs_core::tracing::error!(
                        target: nest_rs_core::target::APP,
                        transport = name,
                        error = %nest_rs_core::error_message(&*error),
                        "a transport the harness spawned failed",
                    );
                    Err(error)
                }
                Err(payload) => {
                    nest_rs_core::contained_panic!(
                        target: nest_rs_core::target::APP,
                        payload.as_ref(),
                        "a transport the harness spawned panicked",
                        transport = name,
                    );
                    Err(anyhow!("transport `{name}` panicked"))
                }
            }
        });
        still.push(Spawned {
            task: join.id(),
            name,
            stop_bound,
            abort: join.abort_handle(),
        });
        drop(still);
        TransportHandle { cancel, join }
    }

    /// Stop the app as [`App::close`] does: it turns
    /// [`Draining`](nest_rs_core::AppState::Draining), the transports it spawned
    /// stop within the longest of their stop bounds plus
    /// [`SHUTDOWN_SETTLE_TIMEOUT`](nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT) — one
    /// still serving then is abandoned at `error` and aborted, as [`App::run`]
    /// abandons it — then the shutdown phases run in reverse, the resources
    /// close, and the container drops.
    pub async fn close(self) {
        let HeadlessApp {
            app,
            transports,
            serving,
        } = self;
        nest_rs_core::__private::close_after(app, stop_within(&transports, &serving)).await;
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wait for every tracked transport to return once the app's stop token fired,
/// within the longest stop bound plus the settle, as [`App::run`] does; one
/// still serving then is abandoned at `error` and aborted.
async fn stop_within(transports: &TaskTracker, serving: &Serving) {
    transports.close();
    let longest = locked(serving)
        .iter()
        .map(|spawned| spawned.stop_bound)
        .max()
        .unwrap_or_default();
    let held = longest + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT;
    if tokio::time::timeout(held, transports.wait()).await.is_ok() {
        return;
    }
    // Aborted, never awaited: a task blocking its thread would hold the way
    // down past every bound.
    for spawned in locked(serving).drain(..) {
        nest_rs_core::tracing::error!(
            target: nest_rs_core::target::APP,
            transport = spawned.name,
            stop_bound_ms = u64::try_from(spawned.stop_bound.as_millis()).unwrap_or(u64::MAX),
            "transport did not stop within its declared bound: abandoned",
        );
        spawned.abort.abort();
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
