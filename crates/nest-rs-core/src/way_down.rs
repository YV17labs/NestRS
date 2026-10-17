//! The way down: from the signal that asks a process to stop to the moment it
//! exits.
//!
//! The kernel owns the signals from `main`'s first line: `#[nest_rs::main]`
//! installs the `SIGINT` / `SIGTERM` handlers before it polls `main`, so the
//! operating system's default action never runs. The first signal turns every
//! app the process booted [`Draining`](crate::AppState::Draining), with the
//! signal as its [`StopCause`]: a boot stops at its next await, naming the step
//! it waited on, and a serving app winds down. A signal before any app began
//! its boot exits at once (130 / 143) after one `error` line, on stderr while
//! no subscriber can take it. One received once the way down has begun — a
//! second signal, or a first after a transport failed — exits at once after
//! one `error` line naming what it abandons. An app run outside
//! `#[nest_rs::main]`, as a `#[tokio::test]` runs it, installs the same
//! handlers for its own run.
//!
//! Dropping a tokio runtime waits for every blocking task still running, so
//! [`main`] tears it down within what the shutdown hooks left of
//! [`SHUTDOWN_HOOKS_TIMEOUT`](crate::SHUTDOWN_HOOKS_TIMEOUT), whether `main`
//! returns or panics. Every exit it controls — that end, the unwind, a signal
//! exiting at once — then writes what the console still queues, within its own
//! bound.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use crate::app::BootStep;
use crate::app_lifecycle::{AppLifecycle, StopCause};
use crate::lifecycle::LifecyclePhase;
use crate::millis::millis;

/// Where the way down stands, for the line a signal exiting at once files —
/// process-wide, as the signals are.
static WAY_DOWN: Mutex<Step> = Mutex::new(Step::Main);

/// What the process is waiting on.
enum Step {
    /// `main`, before any app began its boot.
    Main,
    /// An app's boot, waiting on this step; `None` between two.
    Boot(Option<BootStep>),
    /// The transports, which these have not returned from yet.
    Transports(Vec<Arc<str>>),
    /// One shutdown hook.
    Hook {
        phase: LifecyclePhase,
        provider: &'static str,
        method: &'static str,
        origin: &'static str,
    },
    /// One resource's close, after every shutdown hook.
    Close(String),
    /// The app's teardown is done but for its container dropping; what remains
    /// is that drop, `main`'s locals and the runtime's teardown.
    Exit,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn set(step: Step) {
    *locked(&WAY_DOWN) = step;
}

/// Record the boot step an app waits on, `None` once it is done.
pub(crate) fn boot_step(step: Option<&BootStep>) {
    set(Step::Boot(step.cloned()));
}

pub(crate) fn transports(running: Vec<Arc<str>>) {
    set(Step::Transports(running));
}

pub(crate) fn hook(
    phase: LifecyclePhase,
    provider: &'static str,
    method: &'static str,
    origin: &'static str,
) {
    set(Step::Hook {
        phase,
        provider,
        method,
        origin,
    });
}

pub(crate) fn close(name: &str) {
    set(Step::Close(name.to_owned()));
}

pub(crate) fn exit() {
    set(Step::Exit);
}

/// Exit now, after one `error` naming what the process abandons.
fn exit_at_once(signal: Received) -> ! {
    let code = signal.exit_code();
    let signal = signal.name();
    macro_rules! abandoning {
        ($($field:tt)*) => {
            tracing::error!(
                target: crate::target::APP,
                signal,
                exit_code = code,
                $($field)*,
                "shutdown signal received on the way down: exiting at once, abandoning what still \
                 runs",
            )
        };
    }
    {
        let step = locked(&WAY_DOWN);
        match &*step {
            Step::Main => before_any_app(signal, code),
            Step::Boot(None) => abandoning!(step = "boot"),
            Step::Boot(Some(BootStep::Factory { type_name })) => {
                abandoning!(step = "boot", type_name)
            }
            Step::Boot(Some(
                BootStep::Configure { transport } | BootStep::Listen { transport },
            )) => {
                abandoning!(step = "boot", transport = transport.as_str())
            }
            Step::Boot(Some(BootStep::InitHook {
                phase,
                provider,
                method,
                origin,
            })) => abandoning!(step = "boot", ?phase, provider, method, origin),
            Step::Transports(running) => {
                abandoning!(
                    step = "transports",
                    transports = running.join(", ").as_str()
                )
            }
            Step::Hook {
                phase,
                provider,
                method,
                origin,
            } => abandoning!(step = "shutdown hooks", ?phase, provider, method, origin),
            Step::Close(close) => abandoning!(step = "resource closes", close = close.as_str()),
            Step::Exit => abandoning!(step = "exit"),
        }
    }
    flush_console();
    std::process::exit(code)
}

/// Whether `#[nest_rs::main]` runs this process, and so owns its exits.
static UNDER_MAIN: AtomicBool = AtomicBool::new(false);

/// Whether `#[nest_rs::main]` runs this process: only then is the console
/// queued, since only then is every exit one that writes what it queues.
#[cfg(feature = "logging")]
pub(crate) fn under_main() -> bool {
    UNDER_MAIN.load(Ordering::Relaxed)
}

/// Write what the console still queues, within its bound.
fn flush_console() {
    #[cfg(feature = "logging")]
    crate::logging::flush_console();
}

/// The line a signal before any app files: no subscriber may exist yet, so it
/// goes to stderr when none can take it, as a process step's does.
fn before_any_app(signal: &'static str, code: i32) {
    let file = || {
        tracing::error!(
            target: crate::target::APP,
            signal,
            exit_code = code,
            step = "main",
            "shutdown signal received before the app started: exiting at once",
        );
    };
    if crate::panic::subscribed() {
        file();
    } else {
        tracing::subscriber::with_default(crate::stderr_lines::StderrLines, file);
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

    fn cause(self) -> StopCause {
        match self {
            Self::Interrupt => StopCause::Interrupt,
            Self::Terminate => StopCause::Terminate,
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
    /// Installed where they are created: a signal before a task first polls
    /// them would meet the OS default action and kill the process silently.
    fn install() -> Option<Self> {
        Self::open()
            .inspect_err(|error| {
                tracing::warn!(
                    target: crate::target::APP,
                    error = %crate::error_message(error),
                    "failed to install the shutdown signal handlers; the process stops only when \
                     a transport does",
                );
            })
            .ok()
    }

    #[cfg(unix)]
    fn open() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    #[cfg(not(unix))]
    fn open() -> std::io::Result<Self> {
        Ok(Self {
            ctrl_c: tokio::signal::windows::ctrl_c()?,
        })
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

/// The apps this process booted, for the watcher `#[nest_rs::main]` runs.
struct Process {
    /// Every app whose boot began, while it lives.
    apps: Vec<Weak<AppLifecycle>>,
    /// The first signal, once it reached an app: an app booting after it
    /// starts draining.
    asked: Option<StopCause>,
    /// Whether the watcher runs, so an app's run installs no handlers of its own.
    watched: bool,
}

static PROCESS: Mutex<Process> = Mutex::new(Process {
    apps: Vec::new(),
    asked: None,
    watched: false,
});

/// An app's boot begins: the process's signals now reach it.
pub(crate) fn booting(lifecycle: &Arc<AppLifecycle>) {
    set(Step::Boot(None));
    let asked = {
        let mut process = locked(&PROCESS);
        process.apps.retain(|app| app.strong_count() > 0);
        process.apps.push(Arc::downgrade(lifecycle));
        process.asked
    };
    if let Some(cause) = asked {
        lifecycle.drain(cause);
    }
}

/// Whether `#[nest_rs::main]`'s watcher runs.
pub(crate) fn watched() -> bool {
    locked(&PROCESS).watched
}

/// The watcher `#[nest_rs::main]` runs, until its runtime is torn down.
struct Watching;

impl Drop for Watching {
    fn drop(&mut self) {
        locked(&PROCESS).watched = false;
    }
}

/// Watch the process's signals for every app it boots: the first drains them
/// — or exits at once when none was left to drain — and the next exits at once.
fn watch_process() -> Option<Watching> {
    let mut signals = Signals::install()?;
    locked(&PROCESS).watched = true;
    tokio::spawn(async move {
        let first = signals.next().await;
        let apps: Vec<Arc<AppLifecycle>> = {
            let mut process = locked(&PROCESS);
            let apps: Vec<_> = process.apps.iter().filter_map(Weak::upgrade).collect();
            if !apps.is_empty() {
                process.asked = Some(first.cause());
            }
            apps
        };
        let mut drained = false;
        for app in &apps {
            drained |= app.drain(first.cause());
        }
        if !drained {
            exit_at_once(first);
        }
        let second = signals.next().await;
        exit_at_once(second);
    });
    Some(Watching)
}

/// Watch the process's signals for one app's run, outside `#[nest_rs::main]`:
/// the first drains it, unless it already is; the next exits at once.
pub(crate) fn watch_app(lifecycle: &Arc<AppLifecycle>) {
    let Some(mut signals) = Signals::install() else {
        return;
    };
    let app = Arc::downgrade(lifecycle);
    let stopping = lifecycle.stopping();
    tokio::spawn(async move {
        tokio::select! {
            signal = signals.next() => {
                if !app.upgrade().is_some_and(|app| app.drain(signal.cause())) {
                    exit_at_once(signal);
                }
            }
            () = stopping.cancelled() => {}
        }
        let signal = signals.next().await;
        exit_at_once(signal);
    });
}

/// When the shutdown hooks' budget runs out — the later, should a process run
/// two apps. Read by [`main`] once the app's body has returned.
static HOOKS_DEADLINE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Record that the shutdown hooks' budget runs out `left` from now — on the
/// real clock, never a runtime's, which a test may have paused.
pub(crate) fn hooks_deadline(left: Duration) {
    let deadline = std::time::Instant::now() + left;
    let mut recorded = locked(&HOOKS_DEADLINE);
    *recorded = Some(recorded.map_or(deadline, |earlier| earlier.max(deadline)));
}

/// What remains of the shutdown hooks' budget, the time the runtime's
/// teardown may take — all of it when no app ran.
fn teardown_budget() -> Duration {
    locked(&HOOKS_DEADLINE).map_or(crate::SHUTDOWN_HOOKS_TIMEOUT, |deadline| {
        deadline.saturating_duration_since(std::time::Instant::now())
    })
}

/// What `#[nest_rs::main]` expands to: install the process panic hook, run the
/// process steps while no runtime exists, build the runtime, install the
/// signal handlers on it, run `main` on it, tear it down within what remains
/// of the shutdown hooks' budget, and write what the console still queues.
///
/// The runtime is what `#[tokio::main]` builds; only its teardown is bounded.
pub fn main<T, F: Future<Output = T>>(main: F) -> T {
    UNDER_MAIN.store(true, Ordering::Relaxed);
    std::panic::set_hook(Box::new(crate::panic::hook));
    crate::process_start::run_all();
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
    let watching = {
        let _entered = runtime.enter();
        watch_process()
    };
    // Caught so a panicking `main` gets the same bounded teardown: unwinding
    // past it would drop the runtime, which waits on blocking work unbounded.
    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(main)));
    let budget = teardown_budget();
    let started = std::time::Instant::now();
    runtime.shutdown_timeout(budget);
    // `shutdown_timeout` returns early once every thread stops, so reaching a
    // non-zero budget means work was abandoned; a zero one cannot tell.
    if !budget.is_zero() && started.elapsed() >= budget {
        tracing::warn!(
            target: crate::target::APP,
            budget_ms = millis(budget),
            "work still running as the runtime is torn down is abandoned: the exit no longer \
             waits for it",
        );
    }
    drop(watching);
    flush_console();
    ran.unwrap_or_else(|payload| std::panic::resume_unwind(payload))
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
    fn a_main_that_panics_is_torn_down_within_what_the_hooks_left_and_keeps_its_panic() {
        let logs = LogCapture::install();
        let left = Duration::from_millis(300);
        hooks_deadline(left);
        let started = Instant::now();

        let unwound = std::panic::catch_unwind(|| {
            main::<(), _>(async {
                leave_blocking_work_behind().await;
                panic!("main gave up");
            })
        })
        .expect_err("the panic goes on out of `main`");

        let took = started.elapsed();
        assert!(
            took < left + Duration::from_secs(1),
            "the teardown waited out what the hooks left, not the work ({BLOCKS_FOR:?}): took \
             {took:?}",
        );
        assert_eq!(crate::panic_message(unwound.as_ref()), "main gave up");
        logs.expect_one(crate::target::APP, ABANDONED);
        logs.expect_one(
            crate::target::APP,
            "panicked where no unit of work contains it",
        );
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
        let recorded = locked(&HOOKS_DEADLINE).expect("the run recorded its deadline");
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
