//! [`AppLifecycle`] — where a running [`App`](crate::App) stands and why it
//! stops, injectable by any provider; only the kernel moves it.

use std::future::Future;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::app::BootStep;
use crate::error::BootInterruptedError;

/// Where the app stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AppState {
    /// From the start of the boot until its transports serve.
    Starting,
    /// The boot is done and the transports serve.
    Ready,
    /// A stop was asked; serving winds down.
    Draining,
}

/// Why the app stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopCause {
    /// `SIGINT`, or Ctrl-C on Windows.
    Interrupt,
    /// `SIGTERM`.
    Terminate,
    /// A transport returned `Err`, panicked, or stopped serving on its own —
    /// an app left with no transport serving stops for this cause.
    TransportFailed,
    /// The boot failed — a transport's `configure`, an init hook — before the
    /// app served.
    BootFailed,
    /// [`App::close`](crate::App::close) ended an app that was not serving.
    Closed,
}

impl StopCause {
    /// The signal's name, for a cause a signal is.
    pub(crate) fn signal(self) -> Option<&'static str> {
        match self {
            Self::Interrupt => Some("SIGINT"),
            Self::Terminate => Some("SIGTERM"),
            Self::TransportFailed | Self::BootFailed | Self::Closed => None,
        }
    }
}

/// The state and the cause, moved together so that no reader sees one without
/// the other.
#[derive(Clone, Copy, Debug)]
enum Stand {
    Starting,
    Ready,
    Draining(StopCause),
}

/// The app's lifecycle, injectable as `Arc<AppLifecycle>`: where it stands, why
/// it stops, and the token its stop cancels. One per [`App`](crate::App),
/// seeded before any factory runs, so a factory, a constructor or a hook may
/// read it; only the kernel moves it.
///
/// A loop a provider runs beside the app stops at [`stopping`](Self::stopping)
/// — the instant a stop is asked, before the transports drain — and is joined
/// in the provider's own shutdown hook, since a constructor cannot await:
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// use nest_rs_core::{App, AppLifecycle, hooks, injectable, module};
/// use tokio::sync::Mutex;
/// use tokio::task::JoinHandle;
///
/// #[injectable]
/// struct Heartbeat {
///     #[inject]
///     lifecycle: Arc<AppLifecycle>,
///     beating: Mutex<Option<JoinHandle<u32>>>,
/// }
///
/// #[hooks]
/// impl Heartbeat {
///     #[on_application_bootstrap]
///     async fn start(&self) {
///         let stopping = self.lifecycle.stopping();
///         let beating = tokio::spawn(async move {
///             let mut beats = 0;
///             loop {
///                 tokio::select! {
///                     () = stopping.cancelled() => return beats,
///                     () = tokio::time::sleep(Duration::from_secs(10)) => beats += 1,
///                 }
///             }
///         });
///         *self.beating.lock().await = Some(beating);
///     }
///
///     #[on_module_destroy]
///     async fn stop(&self) -> anyhow::Result<()> {
///         if let Some(beating) = self.beating.lock().await.take() {
///             beating.await?;
///         }
///         Ok(())
///     }
/// }
///
/// #[module(providers = [Heartbeat])]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = App::builder().module::<AppModule>().build().await?;
/// # let heartbeat = app.container().get::<Heartbeat>();
/// # app.run().await?;
/// # assert!(heartbeat.is_some_and(|h| h.beating.try_lock().is_ok_and(|b| b.is_none())));
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct AppLifecycle {
    stand: watch::Sender<Stand>,
    stop: CancellationToken,
}

impl AppLifecycle {
    pub(crate) fn new() -> Self {
        Self {
            stand: watch::Sender::new(Stand::Starting),
            stop: CancellationToken::new(),
        }
    }

    /// Where the app stands now.
    pub fn state(&self) -> AppState {
        match *self.stand.borrow() {
            Stand::Starting => AppState::Starting,
            Stand::Ready => AppState::Ready,
            Stand::Draining(_) => AppState::Draining,
        }
    }

    /// Resolves once the app is [`Ready`](AppState::Ready); `false` once it is
    /// draining, whether or not it served first.
    pub async fn ready(&self) -> bool {
        let mut stand = self.stand.subscribe();
        stand
            .wait_for(|stand| !matches!(stand, Stand::Starting))
            .await
            .is_ok_and(|stand| matches!(*stand, Stand::Ready))
    }

    /// A child of the app's stop token, cancelled the instant the app turns
    /// [`Draining`](AppState::Draining).
    pub fn stopping(&self) -> CancellationToken {
        self.stop.child_token()
    }

    /// Why the app stops: `Some` once it is draining, the first cause asked.
    pub fn cause(&self) -> Option<StopCause> {
        match *self.stand.borrow() {
            Stand::Draining(cause) => Some(cause),
            Stand::Starting | Stand::Ready => None,
        }
    }

    /// `Starting → Ready`, said once; an app already draining stays so.
    pub(crate) fn turn_ready(&self, transports: &str) {
        let turned = self.stand.send_if_modified(|stand| match stand {
            Stand::Starting => {
                *stand = Stand::Ready;
                true
            }
            Stand::Ready | Stand::Draining(_) => false,
        });
        if turned {
            tracing::info!(target: crate::target::APP, transports, "app ready");
        }
    }

    /// `Starting | Ready → Draining`, the first cause winning, then the stop
    /// token cancelled; `false` when the app was already draining.
    pub(crate) fn drain(&self, cause: StopCause) -> bool {
        let drained = self.stand.send_if_modified(|stand| match stand {
            Stand::Starting | Stand::Ready => {
                *stand = Stand::Draining(cause);
                true
            }
            Stand::Draining(_) => false,
        });
        if drained {
            tracing::info!(
                target: crate::target::APP,
                ?cause,
                signal = cause.signal(),
                "app draining",
            );
            self.stop.cancel();
        }
        drained
    }

    /// The token every transport serves under: cancelled with the stop, never
    /// by a transport for the others.
    pub(crate) fn serving(&self) -> CancellationToken {
        self.stop.child_token()
    }

    /// Await `step`'s future unless a stop is asked first: the boot step is
    /// then dropped where it waits, and the boot returns the interruption.
    pub(crate) async fn boot<T>(
        &self,
        step: &BootStep,
        future: impl Future<Output = T>,
    ) -> Result<T, BootInterruptedError> {
        crate::way_down::boot_step(Some(step));
        let outcome = tokio::select! {
            biased;
            cause = self.stopped() => Err(cause),
            done = future => Ok(done),
        };
        crate::way_down::boot_step(None);
        outcome.map_err(|cause| {
            tracing::warn!(
                target: crate::target::APP,
                signal = cause.signal(),
                ?cause,
                step = %step,
                "shutdown signal received during the boot: stopping it",
            );
            BootInterruptedError {
                cause,
                step: step.clone(),
            }
        })
    }

    /// Why the app stops, once it does.
    async fn stopped(&self) -> StopCause {
        let mut stand = self.stand.subscribe();
        loop {
            if let Stand::Draining(cause) = *stand.borrow_and_update() {
                return cause;
            }
            if stand.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use nest_rs_testing::LogCapture;

    use super::*;

    #[tokio::test]
    async fn an_app_turns_ready_then_drains_for_the_first_cause_asked() {
        let logs = LogCapture::install();
        let lifecycle = AppLifecycle::new();
        let stopping = lifecycle.stopping();
        assert_eq!(lifecycle.state(), AppState::Starting);
        assert_eq!(lifecycle.cause(), None);

        lifecycle.turn_ready("Http, Scheduler");
        lifecycle.turn_ready("Http, Scheduler");
        assert_eq!(lifecycle.state(), AppState::Ready);
        assert!(lifecycle.ready().await);
        assert!(!stopping.is_cancelled());

        assert!(lifecycle.drain(StopCause::Terminate));
        assert!(!lifecycle.drain(StopCause::TransportFailed));
        assert_eq!(lifecycle.state(), AppState::Draining);
        assert_eq!(lifecycle.cause(), Some(StopCause::Terminate));
        assert!(stopping.is_cancelled());
        assert!(!lifecycle.ready().await, "it no longer serves");

        let ready = logs.expect_one(crate::target::APP, "app ready");
        assert_eq!(
            ready.field("transports").as_deref(),
            Some("Http, Scheduler")
        );
        let draining = logs.expect_one(crate::target::APP, "app draining");
        assert_eq!(draining.field("cause").as_deref(), Some("Terminate"));
        assert_eq!(draining.field("signal").as_deref(), Some("SIGTERM"));
    }

    #[tokio::test]
    async fn an_app_draining_before_it_served_never_turns_ready() {
        let lifecycle = AppLifecycle::new();
        assert!(lifecycle.drain(StopCause::Interrupt));
        lifecycle.turn_ready("Http");

        assert_eq!(lifecycle.state(), AppState::Draining);
        assert!(!lifecycle.ready().await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiter_on_ready_wakes_at_the_transition() {
        let lifecycle = Arc::new(AppLifecycle::new());
        let waiting = tokio::spawn({
            let lifecycle = Arc::clone(&lifecycle);
            async move { lifecycle.ready().await }
        });
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!waiting.is_finished(), "still starting");

        lifecycle.turn_ready("Http");
        assert_eq!(waiting.await.ok(), Some(true));
    }

    #[tokio::test]
    async fn a_transport_cancelling_its_own_token_stops_no_other() {
        let lifecycle = AppLifecycle::new();
        let one = lifecycle.serving();
        let other = lifecycle.serving();
        one.cancel();

        assert!(!other.is_cancelled());
        assert!(!lifecycle.stopping().is_cancelled());
        assert_eq!(lifecycle.state(), AppState::Starting);
    }

    #[tokio::test(start_paused = true)]
    async fn a_boot_step_waiting_when_the_stop_is_asked_is_dropped_and_named() {
        let logs = LogCapture::install();
        let lifecycle = Arc::new(AppLifecycle::new());
        let booting = tokio::spawn({
            let lifecycle = Arc::clone(&lifecycle);
            async move {
                lifecycle
                    .boot(
                        &BootStep::Factory {
                            type_name: "app::Pool",
                        },
                        std::future::pending::<()>(),
                    )
                    .await
            }
        });
        tokio::time::sleep(Duration::from_secs(1)).await;
        lifecycle.drain(StopCause::Terminate);

        let interrupted = booting
            .await
            .ok()
            .and_then(Result::err)
            .map(|interrupted| (interrupted.cause, interrupted.step.to_string()));
        assert_eq!(
            interrupted,
            Some((
                StopCause::Terminate,
                "the factory for `app::Pool`".to_owned()
            ))
        );
        let line = logs.expect_one(
            crate::target::APP,
            "shutdown signal received during the boot: stopping it",
        );
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("signal").as_deref(), Some("SIGTERM"));
        assert_eq!(
            line.field("step").as_deref(),
            Some("the factory for `app::Pool`")
        );
    }

    #[tokio::test]
    async fn a_boot_step_already_stopped_never_starts() {
        let lifecycle = AppLifecycle::new();
        lifecycle.drain(StopCause::Interrupt);
        let started = AtomicBool::new(false);

        let outcome = lifecycle
            .boot(
                &BootStep::Configure {
                    transport: "Http".to_owned(),
                },
                async { started.store(true, Ordering::SeqCst) },
            )
            .await;

        assert!(outcome.is_err());
        assert!(!started.load(Ordering::SeqCst));
    }
}
