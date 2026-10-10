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
mod container;
mod error_message;
mod lifecycle;
mod module;
mod net;
mod panic;
mod way_down;

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

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
    lines: Receiver<String>,
    errors: Receiver<String>,
    seen: Vec<String>,
}

impl ChildProcess {
    pub(crate) fn spawn(test: &str, role: &str) -> Self {
        Self::spawn_with(test, role, &[])
    }

    pub(crate) fn spawn_with(test: &str, role: &str, env: &[(String, String)]) -> Self {
        let mut child = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", test, "--nocapture"])
            .env(CHILD_ROLE, role)
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the child starts");
        let lines = read_lines(child.stdout.take().expect("the child's stdout"));
        let errors = read_lines(child.stderr.take().expect("the child's stderr"));
        Self {
            child,
            lines,
            errors,
            seen: Vec::new(),
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
    pub(crate) fn stderr(&self) -> Vec<String> {
        let mut printed = Vec::new();
        while let Ok(line) = self.errors.recv_timeout(Duration::from_millis(500)) {
            printed.push(line);
        }
        printed
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
