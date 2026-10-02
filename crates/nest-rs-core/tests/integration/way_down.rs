//! Covers `src/way_down.rs` — the end of the way down: the runtime's teardown
//! under `#[main]`, and a signal received while the process is already
//! stopping.
//!
//! Both are about the *process*, so both are asserted on one: the teardown
//! through a decorated function, which builds and tears down a runtime of its
//! own, and the signals through a child — this same test binary, re-run on one
//! test — that the parent signals and whose exit it reads. A process that
//! exits at once cannot be asserted on from inside.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use nest_rs_core::{
    App, Container, ContainerBuilder, Module, SHUTDOWN_HOOKS_TIMEOUT, Transport,
    TransportContribution, hooks, injectable, module,
};
use nest_rs_testing::LogCapture;
use tokio_util::sync::CancellationToken;

/// How long the blocking call the stuck hook waits on lasts: past the hooks'
/// budget by more than any scheduling noise, so a teardown that waited for it
/// is told apart from one that did not.
const BLOCKS_FOR: Duration = Duration::from_secs(8);

/// A cleanup awaiting a blocking call that outlasts the budget — a synchronous
/// client wrapped the usual way. The budget drops the hook's future; the
/// blocking thread it was waiting on runs on.
#[injectable]
#[derive(Default)]
struct WaitsOnBlocking;

#[hooks]
impl WaitsOnBlocking {
    #[on_module_destroy]
    async fn flush(&self) {
        let _ = tokio::task::spawn_blocking(|| std::thread::sleep(BLOCKS_FOR)).await;
    }
}

/// No transport: `App::run` goes straight to its way down.
#[module(providers = [WaitsOnBlocking])]
struct WaitsOnBlockingModule;

/// The app as `#[main]` runs it — the decorator under test, on an `async fn`
/// like any binary's `main`.
#[nest_rs_core::main]
async fn run_waits_on_blocking() -> anyhow::Result<()> {
    App::new::<WaitsOnBlockingModule>()?.run().await
}

/// A hook the budget abandons no longer holds the exit. Under `#[tokio::main]`
/// the runtime's drop waited for the blocking call the hook had been awaiting —
/// the probe measured `App::run` returning at five seconds and the process at
/// nine, after its last line. `#[main]` tears the runtime down within what the
/// hooks left of their budget, which they spent: the exit comes with the budget.
#[test]
fn a_hook_abandoned_at_the_budget_no_longer_holds_the_exit() {
    let logs = LogCapture::install_global();
    let started = Instant::now();

    run_waits_on_blocking().expect("the app stops cleanly");

    let took = started.elapsed();
    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "the process ended with the hooks' budget, not with the blocking call ({BLOCKS_FOR:?}): \
         took {took:?}",
    );
    let abandoned = logs.expect_one(
        nest_rs_core::target::LIFECYCLE,
        "shutdown hook abandoned: the shutdown hooks' budget was spent while it waited, and the \
         hooks after it still start",
    );
    assert_eq!(
        abandoned.field("provider").as_deref(),
        Some("WaitsOnBlocking")
    );
}

/// An app with nothing to tear down: no transport, no hook.
#[module(providers = [])]
struct NothingToTearDownModule;

/// Leaves blocking work behind that nobody awaits — a fire-and-forget write —
/// then runs an app whose way down spends none of the hooks' budget.
#[nest_rs_core::main]
async fn leave_blocking_work_behind() -> anyhow::Result<()> {
    tokio::task::spawn_blocking(|| std::thread::sleep(BLOCKS_FOR));
    App::new::<NothingToTearDownModule>()?.run().await
}

/// Work still running as the runtime is torn down is given what the hooks left
/// of their budget — all of it here, since none of them ran — and then abandoned
/// with the process, and said: nothing named it before, since no hook waited on
/// it.
#[test]
fn work_the_teardown_waited_out_is_abandoned_and_said() {
    let logs = LogCapture::install();
    let started = Instant::now();

    leave_blocking_work_behind().expect("the app stops cleanly");

    let took = started.elapsed();
    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT - Duration::from_millis(100)
            && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "the teardown waited out what was left of the hooks' budget, not the work \
         ({BLOCKS_FOR:?}): took {took:?}",
    );
    let abandoned = logs.expect_one(
        nest_rs_core::target::APP,
        "work still running as the runtime is torn down is abandoned: the exit no longer waits \
         for it",
    );
    assert_eq!(abandoned.level, "warn");
    assert!(abandoned.field("budget_ms").is_some(), "{abandoned:#?}");
}

/// Leaves blocking work behind and runs no app — a tool's `main`, spawning a
/// write it never awaits.
#[nest_rs_core::main]
async fn leave_blocking_work_behind_without_an_app() {
    tokio::task::spawn_blocking(|| std::thread::sleep(BLOCKS_FOR));
}

/// A `main` that ran no app spent none of the hooks' budget, so its teardown is
/// given all of it: blocking work it left behind is waited for as long as a hook
/// would be, and named once it outlives that. With no budget recorded the
/// teardown used to be given nothing, and the work was abandoned at once,
/// without a line.
#[test]
fn a_main_that_ran_no_app_gives_what_it_left_behind_the_whole_budget() {
    let logs = LogCapture::install();
    let started = Instant::now();

    leave_blocking_work_behind_without_an_app();

    let took = started.elapsed();
    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "the teardown waited out the hooks' whole budget, not the work ({BLOCKS_FOR:?}) and \
         not nothing: took {took:?}",
    );
    let abandoned = logs.expect_one(
        nest_rs_core::target::APP,
        "work still running as the runtime is torn down is abandoned: the exit no longer waits \
         for it",
    );
    assert_eq!(
        abandoned.field("budget_ms"),
        Some(SHUTDOWN_HOOKS_TIMEOUT.as_millis().to_string()),
    );
}

/// The role a child process plays, read from its environment. Unset — the
/// ordinary run of the suite — the child test has nothing to do.
const CHILD_ROLE: &str = "NEST_RS_CORE_WAY_DOWN_CHILD";

/// What a child prints once its handlers are installed and it serves.
const READY: &str = "WAY-DOWN-CHILD SERVING";

/// What the child's stuck hook prints once it has started.
const HOOK_STARTED: &str = "WAY-DOWN-CHILD HOOK STARTED";

/// Serves until the token fires, after saying it serves: the handlers are
/// installed before any transport serves, so a parent reading this line may
/// signal.
struct ServesUntilStopped;

#[async_trait::async_trait]
impl Transport for ServesUntilStopped {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        println!("{READY}");
        cancel.cancelled().await;
        Ok(())
    }
}

/// Ignores the token: a transport whose stop never comes.
struct NeverStops;

#[async_trait::async_trait]
impl Transport for NeverStops {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        println!("{READY}");
        std::future::pending::<()>().await;
        Ok(())
    }
}

struct NeverStopsModule;

impl Module for NeverStopsModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "NeverStops",
            build: |_| Ok(Box::new(NeverStops)),
        })
    }
}

struct ServesUntilStoppedModule;

impl Module for ServesUntilStoppedModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "ServesUntilStopped",
            build: |_| Ok(Box::new(ServesUntilStopped)),
        })
    }
}

/// A cleanup that never returns, after saying it started.
#[injectable]
#[derive(Default)]
struct HangsOnDestroy;

#[hooks]
impl HangsOnDestroy {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_destroy]
    async fn release(&self) {
        println!("{HOOK_STARTED}");
        std::future::pending::<()>().await;
    }
}

#[module(providers = [HangsOnDestroy])]
struct HangsOnDestroyModule;

#[nest_rs_core::main]
async fn run_child(role: String) -> anyhow::Result<()> {
    match role.as_str() {
        "hook" => {
            App::builder()
                .module::<ServesUntilStoppedModule>()
                .module::<HangsOnDestroyModule>()
                .build()
                .await?
                .run()
                .await
        }
        "transport" => App::new::<NeverStopsModule>()?.run().await,
        other => anyhow::bail!("no child role `{other}`"),
    }
}

/// The child half of the signal tests: runs the app its parent named, and is
/// otherwise a test with nothing to do.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
fn way_down_child_process() {
    if let Ok(role) = std::env::var(CHILD_ROLE) {
        let _ = run_child(role);
    }
}

/// A child process playing `role`, and the lines it prints, as they arrive.
struct ChildProcess {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl ChildProcess {
    fn spawn(role: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", "way_down::way_down_child_process", "--nocapture"])
            .env(CHILD_ROLE, role)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the child starts");
        let stdout = child.stdout.take().expect("the child's stdout");
        let (send, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Wait for a line containing `needle`.
    fn expect_line(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let found = line.contains(needle);
                    self.seen.push(line);
                    if found {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    let _ = self.child.kill();
                    panic!(
                        "the child never printed {needle:?}; it printed {:#?}",
                        self.seen
                    )
                }
            }
        }
    }

    fn signal(&self, name: &str) {
        let sent = Command::new("kill")
            .args([format!("-{name}"), self.child.id().to_string()])
            .status()
            .expect("`kill` runs");
        assert!(sent.success(), "SIG{name} was delivered");
    }

    /// Wait for the child to exit — within `bound`, or the test fails — and
    /// return its code and everything it printed.
    fn exit_within(&mut self, bound: Duration) -> (Option<i32>, Vec<String>) {
        let asked = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("the child's status") {
                while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(500)) {
                    self.seen.push(line);
                }
                return (status.code(), std::mem::take(&mut self.seen));
            }
            if asked.elapsed() > bound {
                let _ = self.child.kill();
                panic!(
                    "the child was still running {bound:?} after the second signal; it printed \
                     {:#?}",
                    self.seen
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A child is never left running past its test, whichever way the test ends.
impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The line a signal on the way down files.
const EXITING: &str =
    "shutdown signal received on the way down: exiting at once, abandoning what still runs";

/// A second signal while a shutdown hook hangs exits at once — well inside the
/// hooks' budget — with `SIGINT`'s code, after a line naming the hook it
/// abandons. Before, the handlers the first signal installed swallowed it: the
/// process sat out the budget, and a hook that blocked its thread held it until
/// the orchestrator's kill.
#[cfg(unix)]
#[test]
fn a_second_signal_while_a_hook_hangs_exits_at_once_naming_the_hook() {
    let mut child = ChildProcess::spawn("hook");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line(HOOK_STARTED);
    child.signal("INT");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(code, Some(130), "SIGINT's code: {printed:#?}");
    let line = printed
        .iter()
        .find(|line| line.contains(EXITING))
        .unwrap_or_else(|| panic!("the line naming what it abandons: {printed:#?}"));
    assert!(line.contains("HangsOnDestroy"), "{line}");
    assert!(line.contains("release"), "{line}");
    assert!(line.contains("SIGINT"), "{line}");
}

/// The same, while a transport refuses to stop: the line names it, and the code
/// is `SIGTERM`'s — what an orchestrator repeating its `SIGTERM` reads back.
#[cfg(unix)]
#[test]
fn a_second_signal_while_a_transport_will_not_stop_exits_at_once_naming_it() {
    let mut child = ChildProcess::spawn("transport");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line("shutdown signal received");
    child.signal("TERM");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(code, Some(143), "SIGTERM's code: {printed:#?}");
    let line = printed
        .iter()
        .find(|line| line.contains(EXITING))
        .unwrap_or_else(|| panic!("the line naming what it abandons: {printed:#?}"));
    assert!(line.contains("NeverStops"), "{line}");
    assert!(line.contains("SIGTERM"), "{line}");
}
