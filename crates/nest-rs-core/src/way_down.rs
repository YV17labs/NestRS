//! The way down: from the signal that asks a process to stop to the moment it
//! exits.
//!
//! The first `SIGINT` or `SIGTERM` asks for a graceful stop; one received once
//! the way down has begun exits at once (130 / 143) after one `error` line
//! naming what it abandons — the runtime owns the handlers by then, so the
//! operating system's default action no longer runs.
//!
//! Dropping a tokio runtime waits for every blocking task still running, so
//! [`main`] tears it down within what the shutdown hooks left of
//! [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT).

use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::lifecycle::LifecyclePhase;

/// Where the way down stands, for the line a signal received during it files.
#[derive(Default)]
pub(crate) struct WayDown(Mutex<Step>);

/// What the way down is waiting on.
#[derive(Default)]
enum Step {
    /// The transports, which these have not returned from yet.
    Transports(Vec<&'static str>),
    /// One shutdown hook.
    Hook {
        phase: LifecyclePhase,
        provider: &'static str,
        method: &'static str,
    },
    /// [`App::run`](crate::App::run) has returned; what remains is `main`'s
    /// locals dropping and the runtime's teardown.
    #[default]
    Exit,
}

impl WayDown {
    fn set(&self, step: Step) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = step;
    }

    pub(crate) fn transports(&self, running: Vec<&'static str>) {
        self.set(Step::Transports(running));
    }

    pub(crate) fn hook(&self, phase: LifecyclePhase, provider: &'static str, method: &'static str) {
        self.set(Step::Hook {
            phase,
            provider,
            method,
        });
    }

    pub(crate) fn exit(&self) {
        self.set(Step::Exit);
    }

    fn exit_at_once(&self, signal: Received) -> ! {
        let code = signal.exit_code();
        {
            let step = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            match &*step {
                Step::Transports(running) => tracing::error!(
                    target: crate::target::APP,
                    signal = signal.name(),
                    exit_code = code,
                    step = "transports",
                    transports = running.join(", ").as_str(),
                    "shutdown signal received on the way down: exiting at once, abandoning what \
                     still runs",
                ),
                Step::Hook {
                    phase,
                    provider,
                    method,
                } => tracing::error!(
                    target: crate::target::APP,
                    signal = signal.name(),
                    exit_code = code,
                    step = "shutdown hooks",
                    ?phase,
                    provider,
                    method,
                    "shutdown signal received on the way down: exiting at once, abandoning what \
                     still runs",
                ),
                Step::Exit => tracing::error!(
                    target: crate::target::APP,
                    signal = signal.name(),
                    exit_code = code,
                    step = "exit",
                    "shutdown signal received on the way down: exiting at once, abandoning what \
                     still runs",
                ),
            }
        }
        std::process::exit(code)
    }
}

/// One of the two signals that ask a process to stop.
#[derive(Clone, Copy)]
enum Received {
    Interrupt,
    Terminate,
}

impl Received {
    fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }

    /// `128 + n`, the status a POSIX shell reports for a process killed by
    /// signal `n`.
    fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
}

/// Every stop signal the process can receive, as one stream — installed once,
/// so a signal arriving between two waits is queued rather than missed.
struct Signals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(not(unix))]
    ctrl_c: tokio::signal::windows::CtrlC,
}

impl Signals {
    fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                ctrl_c: tokio::signal::windows::ctrl_c()?,
            })
        }
    }

    /// The next stop signal; pending forever once the runtime shuts the
    /// streams down.
    async fn next(&mut self) -> Received {
        #[cfg(unix)]
        {
            tokio::select! {
                Some(()) = self.interrupt.recv() => Received::Interrupt,
                Some(()) = self.terminate.recv() => Received::Terminate,
                else => std::future::pending().await,
            }
        }
        #[cfg(not(unix))]
        {
            match self.ctrl_c.recv().await {
                Some(()) => Received::Interrupt,
                None => std::future::pending().await,
            }
        }
    }
}

/// Watch for the signals that stop the process: the first cancels `cancel`;
/// one received once `cancel` is cancelled exits at once.
///
/// The handlers are installed here, not in the spawned task: a signal before
/// its first poll would meet the OS default action and kill the process silently.
pub(crate) fn watch_signals(cancel: CancellationToken, way_down: std::sync::Arc<WayDown>) {
    let mut signals = match Signals::install() {
        Ok(signals) => signals,
        Err(error) => {
            tracing::warn!(
                target: crate::target::APP,
                error = %crate::error_message(&error),
                "failed to install the shutdown signal handlers; the process stops only when a \
                 transport does",
            );
            return;
        }
    };
    tokio::spawn(async move {
        tokio::select! {
            signal = signals.next() => {
                tracing::info!(
                    target: crate::target::APP,
                    signal = signal.name(),
                    "shutdown signal received",
                );
                cancel.cancel();
            }
            () = cancel.cancelled() => {}
        }
        let signal = signals.next().await;
        way_down.exit_at_once(signal);
    });
}

/// When the shutdown hooks' budget runs out — the later, should a process run
/// two apps. Read by [`main`] once the app's body has returned.
static HOOKS_DEADLINE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Record that the shutdown hooks' budget runs out `left` from now — on the
/// real clock, never a runtime's, which a test may have paused.
pub(crate) fn hooks_deadline(left: Duration) {
    let deadline = std::time::Instant::now() + left;
    let mut recorded = HOOKS_DEADLINE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *recorded = Some(recorded.map_or(deadline, |earlier| earlier.max(deadline)));
}

/// What remains of the shutdown hooks' budget, the time the runtime's
/// teardown may take — all of it when no app ran.
fn teardown_budget() -> Duration {
    HOOKS_DEADLINE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .map_or(crate::SHUTDOWN_HOOKS_TIMEOUT, |deadline| {
            deadline.saturating_duration_since(std::time::Instant::now())
        })
}

/// What `#[nest_rs::main]` expands to: build the runtime, run `main` on it, and
/// tear it down within what remains of the shutdown hooks' budget.
///
/// The runtime is what `#[tokio::main]` builds; only its teardown is bounded.
pub fn main<T, F: Future<Output = T>>(main: F) -> T {
    #[expect(
        clippy::panic,
        reason = "without a runtime `main` cannot start, and nothing above it can be told"
    )]
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            panic!("#[nest_rs::main] could not build the tokio runtime `main` runs on: {error}")
        });
    let output = runtime.block_on(main);
    let budget = teardown_budget();
    let started = std::time::Instant::now();
    runtime.shutdown_timeout(budget);
    // `shutdown_timeout` returns early once every thread stops, so reaching a
    // non-zero budget means work was abandoned; a zero one cannot tell.
    if !budget.is_zero() && started.elapsed() >= budget {
        tracing::warn!(
            target: crate::target::APP,
            budget_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
            "work still running as the runtime is torn down is abandoned: the exit no longer \
             waits for it",
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::{App, ContainerBuilder, Module, Registering};

    /// Past every budget a test records by more than scheduling noise.
    const BLOCKS_FOR: Duration = Duration::from_secs(8);

    const ABANDONED: &str = "work still running as the runtime is torn down is abandoned: the exit \
                             no longer waits for it";

    async fn leave_blocking_work_behind() {
        tokio::task::spawn_blocking(|| std::thread::sleep(BLOCKS_FOR));
    }

    #[test]
    fn work_outliving_what_the_hooks_left_is_abandoned_at_it_and_said() {
        let logs = LogCapture::install();
        let left = Duration::from_millis(300);
        hooks_deadline(left);
        let started = Instant::now();

        main(leave_blocking_work_behind());

        let took = started.elapsed();
        assert!(
            took >= left - Duration::from_millis(100) && took < left + Duration::from_secs(1),
            "the teardown waited out what the hooks left, not the work ({BLOCKS_FOR:?}): took \
             {took:?}",
        );
        let abandoned = logs.expect_one(crate::target::APP, ABANDONED);
        assert_eq!(abandoned.level, "warn");
        let budget_ms: u128 = abandoned
            .field("budget_ms")
            .and_then(|ms| ms.parse().ok())
            .expect("the budget it was given");
        assert!(budget_ms <= left.as_millis(), "{abandoned:#?}");
    }

    #[test]
    fn a_teardown_the_hooks_left_nothing_holds_the_exit_for_nothing() {
        let logs = LogCapture::install();
        hooks_deadline(Duration::ZERO);
        let started = Instant::now();

        main(leave_blocking_work_behind());

        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "took {took:?}");
        assert!(logs.find(crate::target::APP, ABANDONED).is_empty());
    }

    #[test]
    fn a_main_that_ran_no_app_gives_what_it_left_behind_the_whole_budget() {
        assert_eq!(teardown_budget(), crate::SHUTDOWN_HOOKS_TIMEOUT);
        let written = Arc::new(AtomicBool::new(false));
        let writes = Arc::clone(&written);

        main(async move {
            tokio::task::spawn_blocking(move || {
                std::thread::sleep(Duration::from_millis(100));
                writes.store(true, Ordering::SeqCst);
            });
        });

        assert!(
            written.load(Ordering::SeqCst),
            "the write it left behind was waited for"
        );
    }

    struct NothingToTearDown;
    impl Module for NothingToTearDown {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }
    }

    #[tokio::test]
    async fn an_app_run_records_where_its_hooks_budget_ends() {
        let ran = Instant::now();
        App::new::<NothingToTearDown>()
            .expect("boots")
            .run()
            .await
            .expect("stops cleanly");
        let recorded = HOOKS_DEADLINE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .expect("the run recorded its deadline");
        assert!(
            recorded >= ran + crate::SHUTDOWN_HOOKS_TIMEOUT
                && recorded <= Instant::now() + crate::SHUTDOWN_HOOKS_TIMEOUT,
            "{:?} after the run started",
            recorded - ran,
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_paused_clock_moved_on_records_no_deadline_past_the_budget() {
        tokio::time::advance(Duration::from_secs(60 * 60)).await;
        App::new::<NothingToTearDown>()
            .expect("boots")
            .run()
            .await
            .expect("stops cleanly");
        assert!(
            teardown_budget() <= crate::SHUTDOWN_HOOKS_TIMEOUT,
            "{:?}",
            teardown_budget()
        );
    }
}
