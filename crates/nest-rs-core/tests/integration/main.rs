//! Integration tests mirroring `src/`.
//!
//! What a process does on its way out cannot be asserted from inside it, so
//! [`ChildProcess`] re-runs this same test binary on one test, handing it a role
//! through the environment, and reads what it prints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]
mod access;
mod app;
mod app_lifecycle;
mod container;
mod discoverable;
mod error_message;
mod exception_filter;
mod interceptor;
mod lifecycle;
#[cfg(feature = "logging")]
mod logging;
mod module;
mod net;
mod panic;
mod process_start;
mod way_down;
mod wiring;

use std::io::{BufRead, BufReader, Read, Write as _};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use nest_rs_core::{App, Container, Module, Transport};
use tokio_util::sync::CancellationToken;

/// The boot of `M` on both paths, each expected to fail.
pub(crate) async fn refused_on_both_paths<M: Module>() -> [anyhow::Error; 2] {
    let Err(sync) = App::new::<M>() else {
        panic!("`App::new` boots {}", std::any::type_name::<M>());
    };
    let Err(built) = App::builder().module::<M>().build().await else {
        panic!("`App::builder()` boots {}", std::any::type_name::<M>());
    };
    [sync, built]
}

/// A transport whose `serve` fails at once, as if its listener broke.
pub(crate) struct Failing;

#[async_trait::async_trait]
impl Transport for Failing {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("the listener stopped accepting"))
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

/// Rust's own panic hook back once a `#[nest_rs_core::main]` run has installed
/// the framework's, so an assertion failing after it prints rather than
/// becoming an event in the test's capture.
pub(crate) fn rust_panic_hook() {
    drop(std::panic::take_hook());
}

/// The variable a child reads its role from.
pub(crate) const CHILD_ROLE: &str = "NEST_RS_CORE_CHILD_ROLE";

/// The role this process was started in, when a parent test started it.
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
pub(crate) fn child_role() -> Option<String> {
    std::env::var(CHILD_ROLE).ok()
}

/// This test binary, re-run on `test` alone with `role`.
pub(crate) struct ChildProcess {
    child: Child,
    /// The child's stdout, while the parent leaves it unread.
    unread: Option<ChildStdout>,
    lines: Receiver<String>,
    errors: Receiver<String>,
    seen: Vec<String>,
    seen_errors: Vec<String>,
}

impl ChildProcess {
    pub(crate) fn spawn(test: &str, role: &str) -> Self {
        Self::spawn_with(test, role, &[])
    }

    pub(crate) fn spawn_with(test: &str, role: &str, env: &[(String, String)]) -> Self {
        let mut child = Self::start(test, role, env, Stdio::inherit());
        child.read_stdout();
        child
    }

    /// A child whose stdout fills and stays full until [`read_stdout`](Self::read_stdout),
    /// and whose stdin is a pipe [`release`](Self::release) writes a line to.
    pub(crate) fn spawn_unread(test: &str, role: &str, env: &[(String, String)]) -> Self {
        Self::start(test, role, env, Stdio::piped())
    }

    fn start(test: &str, role: &str, env: &[(String, String)], stdin: Stdio) -> Self {
        let mut child = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", test, "--nocapture"])
            .env(CHILD_ROLE, role)
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the child starts");
        let unread = child.stdout.take();
        let errors = read_lines(child.stderr.take().expect("the child's stderr"));
        // Nothing reads the child's stdout until `read_stdout` replaces this.
        let (_, lines) = std::sync::mpsc::channel();
        Self {
            child,
            unread,
            lines,
            errors,
            seen: Vec::new(),
            seen_errors: Vec::new(),
        }
    }

    /// Start reading the child's stdout.
    pub(crate) fn read_stdout(&mut self) {
        if let Some(stdout) = self.unread.take() {
            self.lines = read_lines(stdout);
        }
    }

    /// Write one line to the child's stdin, and close it.
    pub(crate) fn release(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            stdin.write_all(b"\n").expect("the child reads its stdin");
        }
    }

    /// The first line the child prints on stderr containing `needle`, within
    /// `bound`.
    pub(crate) fn expect_stderr_line(&mut self, needle: &str, bound: Duration) -> String {
        let deadline = Instant::now() + bound;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.errors.recv_timeout(left) {
                Ok(line) => {
                    self.seen_errors.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    let _ = self.child.kill();
                    panic!(
                        "the child never printed {needle:?} on stderr; it printed {:#?}",
                        self.seen_errors
                    )
                }
            }
        }
    }

    pub(crate) fn expect_line(&mut self, needle: &str) {
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

    pub(crate) fn signal(&self, name: &str) {
        let sent = Command::new("kill")
            .args([format!("-{name}"), self.child.id().to_string()])
            .status()
            .expect("`kill` runs");
        assert!(sent.success(), "SIG{name} was delivered");
    }

    /// The child's exit code and every line it printed on stdout, once it
    /// exits within `bound`.
    pub(crate) fn exit_within(&mut self, bound: Duration) -> (Option<i32>, Vec<String>) {
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
                    "the child was still running {bound:?} after it was asked to stop; it \
                     printed {:#?}",
                    self.seen
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Every line the child printed on stderr; call it once the child exited.
    pub(crate) fn stderr(&mut self) -> Vec<String> {
        while let Ok(line) = self.errors.recv_timeout(Duration::from_millis(500)) {
            self.seen_errors.push(line);
        }
        std::mem::take(&mut self.seen_errors)
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Each line `stream` prints, as it prints it, until it closes.
fn read_lines(stream: impl Read + Send + 'static) -> Receiver<String> {
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    lines
}
