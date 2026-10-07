//! The way down: from the signal that asks a process to stop to the moment it
//! exits, and the two things the kernel owes it beyond the bounded steps
//! [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT) tabulates.
//!
//! **A second signal exits at once.** The first `SIGINT` or `SIGTERM` asks for a
//! graceful stop; one received while the way down is under way — a second
//! `Ctrl-C`, an orchestrator's repeated `SIGTERM`, a signal after a transport's
//! failure began the way down — is a person or a supervisor that has stopped
//! waiting. Since the first, the process's signal handlers have been the
//! runtime's, so the operating system's default action no longer runs: before
//! this, a second signal did nothing at all, and the only way out of a stuck
//! shutdown was `SIGKILL`, which says nothing. Now it exits with the code a
//! shell gives a process killed by that signal — 130 for `SIGINT`, 143 for
//! `SIGTERM` — after one `error` line naming what the way down was still
//! waiting on. Nothing after that line runs: no hook still to start, no
//! telemetry flush, since exiting at once is what was asked.
//!
//! **The runtime's own teardown is bounded too.** Dropping a tokio runtime waits
//! for every blocking task still running, and an abandoned hook or a request
//! dropped at the window may have left one behind — a `spawn_blocking`, a
//! `tokio::fs` call, a synchronous client wrapped the usual way. A hook the
//! budget logged as abandoned at five seconds then held the exit for as long as
//! its blocking call lasted, past every bound and after the last line was
//! written. [`__main`], which `#[nest_rs::main]` expands to, tears the runtime
//! down within what the shutdown hooks and the telemetry flush left of the
//! hooks' budget — all of it when no app ran — so the way down's sum holds to
//! the exit and abandoned work no longer holds it.

use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::lifecycle::LifecyclePhase;

/// Where the way down stands, for the line a signal received during it files.
///
/// One per [`App::run`](crate::App::run), shared with the task that watches for
/// signals: the run writes the step it is on, the watcher reads it only to say
/// what it is about to abandon.
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
    /// Nothing of the app's: [`App::run`](crate::App::run) has returned, and
    /// what remains is the process's own — `main`'s locals dropping, the
    /// telemetry flush among them, then the runtime's teardown.
    #[default]
    Exit,
}

impl WayDown {
    fn set(&self, step: Step) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = step;
    }

    /// The transports still serving, or still stopping.
    pub(crate) fn transports(&self, running: Vec<&'static str>) {
        self.set(Step::Transports(running));
    }

    /// The shutdown hook about to run.
    pub(crate) fn hook(&self, phase: LifecyclePhase, provider: &'static str, method: &'static str) {
        self.set(Step::Hook {
            phase,
            provider,
            method,
        });
    }

    /// The app has stopped.
    pub(crate) fn exit(&self) {
        self.set(Step::Exit);
    }

    /// File the line a signal received on the way down files, naming what is
    /// abandoned, then exit with that signal's code.
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
    /// signal `n` — what a supervisor reading the exit expects of one.
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

    /// The next stop signal. A stream that ends — the runtime shutting down —
    /// never yields one, so the caller waits on it no longer than the runtime
    /// lives.
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

/// Watch for the signals that stop the process: the first cancels `cancel`,
/// which every transport serves until; one received once the way down has begun
/// — after that first, or after a transport's failure cancelled `cancel` itself
/// — exits at once.
///
/// The handlers are installed here, before any transport serves, rather than
/// inside the task that waits on them: a signal arriving between the first
/// request and the task's first poll would otherwise meet the operating
/// system's default action and kill the process without a line.
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

/// When the shutdown hooks' budget runs out, once an app has started spending
/// it — the later, should a process run two. Read by [`__main`] after `main`'s
/// body has returned, which is the only reader, so a process global rather than
/// a value threaded through code the developer writes.
static HOOKS_DEADLINE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Record that the shutdown hooks' budget runs out `left` from now. The
/// deadline is set on the real clock, which the teardown reads, never off a
/// runtime's, which a test may have paused and moved on.
pub(crate) fn hooks_deadline(left: Duration) {
    let deadline = std::time::Instant::now() + left;
    let mut recorded = HOOKS_DEADLINE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *recorded = Some(recorded.map_or(deadline, |earlier| earlier.max(deadline)));
}

/// What remains of the shutdown hooks' budget: the time the runtime's teardown
/// may take.
///
/// **All of it when no app spent any** — a boot that failed, a tool's `main`
/// that never ran one. Nothing of the budget was spent, so nothing of it is
/// owed elsewhere, and blocking work such a `main` left behind — a write it
/// spawned and never awaited — is waited for as long as a hook would be, and
/// named if it outlives that, rather than abandoned unsaid at once.
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
/// **Internal ABI** — the decorator's expansion, lockstep with `nest-rs-core`;
/// write `#[nest_rs::main]` rather than calling it.
///
/// The runtime is tokio's multi-threaded one with every driver enabled — what
/// `#[tokio::main]` builds, sized the same way, by `TOKIO_WORKER_THREADS` or the
/// machine's cores. Its teardown is the difference: work still running once
/// `main` has returned — a blocking call an abandoned hook was waiting on, one a
/// request dropped at the window had started, a write a tool spawned and never
/// awaited — is given what the hooks and the flush left of the hooks' budget,
/// all of it when no app ran, and then abandoned with the process, rather than
/// waited for without a bound. Work abandoned there that was still running is
/// said once, at `warn`.
#[doc(hidden)]
pub fn __main<T, F: Future<Output = T>>(main: F) -> T {
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
    // Measurable only when there was a budget to run out: the runtime's
    // teardown returns early once every thread has stopped, so reaching the
    // budget means something was still running at it. With none left — the
    // hooks and the flush spent it — a teardown that waits not at all cannot
    // tell whether anything was still running, so it says nothing it cannot
    // know; the hooks' own lines say where the budget went.
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
    use crate::{App, ContainerBuilder, Module};

    /// How long the blocking work a test leaves behind lasts: past every budget
    /// a test records by more than any scheduling noise, so a teardown that
    /// waited for it is told apart from one that did not.
    const BLOCKS_FOR: Duration = Duration::from_secs(8);

    const ABANDONED: &str = "work still running as the runtime is torn down is abandoned: the exit \
                             no longer waits for it";

    /// Leaves blocking work behind that nobody awaits — a fire-and-forget
    /// write, or the call an abandoned hook was waiting on.
    async fn leave_blocking_work_behind() {
        tokio::task::spawn_blocking(|| std::thread::sleep(BLOCKS_FOR));
    }

    /// Work still running as the runtime is torn down is given what the hooks
    /// left of their budget, and then abandoned with the process, and said.
    #[test]
    fn work_outliving_what_the_hooks_left_is_abandoned_at_it_and_said() {
        let logs = LogCapture::install();
        let left = Duration::from_millis(300);
        hooks_deadline(left);
        let started = Instant::now();

        __main(leave_blocking_work_behind());

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

    /// Hooks that spent the budget leave the teardown nothing: the blocking call
    /// an abandoned hook was waiting on no longer holds the exit — under
    /// `#[tokio::main]` it held it past its last line — and a teardown that
    /// waited not at all says nothing it cannot know.
    #[test]
    fn a_teardown_the_hooks_left_nothing_holds_the_exit_for_nothing() {
        let logs = LogCapture::install();
        hooks_deadline(Duration::ZERO);
        let started = Instant::now();

        __main(leave_blocking_work_behind());

        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "took {took:?}");
        assert!(logs.find(crate::target::APP, ABANDONED).is_empty());
    }

    /// A `main` that ran no app spent none of the hooks' budget, so its
    /// teardown is given all of it, and blocking work it left behind is waited
    /// for. With no budget recorded the teardown used to be given nothing, and
    /// the work was abandoned at once, without a line.
    #[test]
    fn a_main_that_ran_no_app_gives_what_it_left_behind_the_whole_budget() {
        assert_eq!(teardown_budget(), crate::SHUTDOWN_HOOKS_TIMEOUT);
        let written = Arc::new(AtomicBool::new(false));
        let writes = Arc::clone(&written);

        __main(async move {
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

    /// An app with nothing to tear down: no transport, no hook.
    struct NothingToTearDown;
    impl Module for NothingToTearDown {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder
        }
    }

    /// `App::run` records where the hooks' budget ends as their phases start,
    /// which is what the teardown above is given.
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

    /// The teardown is given real time, so an app whose clock was paused and
    /// moved on — a test's — records no deadline past what the budget allows.
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
