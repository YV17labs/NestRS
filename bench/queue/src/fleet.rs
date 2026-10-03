//! The replicas of one run, each its own process — what a deployment's worker
//! pods are — spawned from this binary and read back over their stdout.

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nest_rs::core::EnvPrefix;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::probe::Run;
use crate::replica::{self, READY};

/// How long a replica may take to build its app.
const BOOT_WITHIN: Duration = Duration::from_secs(30);

/// Linux reports a process's CPU time in ticks of `USER_HZ`, which is 100 on
/// every architecture it runs on.
const TICKS_PER_SECOND: u64 = 100;

pub struct Fleet {
    replicas: Vec<Replica>,
    lines: mpsc::UnboundedReceiver<Line>,
    ready: usize,
}

struct Replica {
    child: Child,
    stdin: Option<ChildStdin>,
    pid: u32,
}

enum Line {
    Ready,
    Ran(Run),
    Gone(usize),
}

/// Each job's first run, by `seq`, as the replicas reported them.
pub struct Ledger {
    first: Vec<Option<Run>>,
    pub duplicates: usize,
    done: usize,
}

impl Ledger {
    pub fn new(jobs: u32) -> Self {
        Self {
            first: vec![None; jobs as usize],
            duplicates: 0,
            done: 0,
        }
    }

    fn note(&mut self, run: Run) {
        match self.first.get_mut(run.seq as usize) {
            Some(slot @ None) => {
                *slot = Some(run);
                self.done += 1;
            }
            Some(Some(_)) => self.duplicates += 1,
            None => {}
        }
    }

    pub fn complete(&self) -> bool {
        self.done == self.first.len()
    }

    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.first.iter().flatten()
    }
}

impl Fleet {
    /// `log` is the framework's log filter in the replicas, set through its own
    /// variable so the shell's cannot change what is measured.
    pub fn spawn(replicas: usize, log: &str) -> Result<Self> {
        let exe = std::env::current_exe().context("finding this binary to spawn replicas")?;
        let (tx, lines) = mpsc::unbounded_channel();
        let mut spawned = Vec::with_capacity(replicas);
        for index in 0..replicas {
            let mut child = Command::new(&exe)
                .arg("replica")
                .env(EnvPrefix::var("LOG"), log)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("spawning a replica")?;
            let stdout = child.stdout.take().context("a replica's stdout")?;
            let stdin = child.stdin.take();
            let pid = child.id().context("a running replica's pid")?;
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let read = if line == READY {
                        Line::Ready
                    } else if let Some(run) = replica::parse_run(&line) {
                        Line::Ran(run)
                    } else {
                        eprintln!("replica {index}: {line}");
                        continue;
                    };
                    if tx.send(read).is_err() {
                        return;
                    }
                }
                let _gone = tx.send(Line::Gone(index));
            });
            spawned.push(Replica { child, stdin, pid });
        }
        Ok(Self {
            replicas: spawned,
            lines,
            ready: 0,
        })
    }

    pub async fn ready(&mut self, ledger: &mut Ledger) -> Result<()> {
        let deadline = Instant::now() + BOOT_WITHIN;
        while self.ready < self.replicas.len() {
            self.next(ledger, deadline).await?;
        }
        Ok(())
    }

    /// Read the replicas' reports until every job of `ledger` ran once.
    pub async fn until_complete(&mut self, ledger: &mut Ledger, within: Duration) -> Result<()> {
        let deadline = Instant::now() + within;
        while !ledger.complete() {
            self.next(ledger, deadline).await?;
        }
        Ok(())
    }

    async fn next(&mut self, ledger: &mut Ledger, deadline: Instant) -> Result<()> {
        let wait = deadline.saturating_duration_since(Instant::now());
        let Ok(line) = timeout(wait, self.lines.recv()).await else {
            bail!(
                "gave up after the run's deadline with {} of {} jobs run",
                ledger.done,
                ledger.first.len()
            );
        };
        match line.context("every replica's reader stopped")? {
            Line::Ready => self.ready += 1,
            Line::Ran(run) => ledger.note(run),
            Line::Gone(index) => bail!("replica {index} exited mid-run"),
        }
        Ok(())
    }

    /// The CPU every replica has spent since it started, user and system.
    pub fn cpu(&self) -> Result<Duration> {
        let mut ticks = 0;
        for replica in &self.replicas {
            ticks += cpu_ticks(replica.pid)?;
        }
        Ok(Duration::from_millis(ticks * 1000 / TICKS_PER_SECOND))
    }

    /// Tell every replica to report the rest and exit, and keep what arrives.
    pub async fn stop(mut self, ledger: &mut Ledger) -> Result<()> {
        for replica in &mut self.replicas {
            drop(replica.stdin.take());
        }
        let deadline = Instant::now() + BOOT_WITHIN;
        let mut gone = 0;
        while gone < self.replicas.len() {
            let wait = deadline.saturating_duration_since(Instant::now());
            match timeout(wait, self.lines.recv()).await {
                Ok(Some(Line::Ran(run))) => ledger.note(run),
                Ok(Some(Line::Gone(_))) => gone += 1,
                Ok(Some(Line::Ready)) => {}
                Ok(None) => break,
                Err(_) => bail!("a replica did not exit once asked"),
            }
        }
        for replica in &mut self.replicas {
            let status = replica.child.wait().await?;
            if !status.success() {
                bail!("a replica exited with {status}");
            }
        }
        Ok(())
    }
}

fn cpu_ticks(pid: u32) -> Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .with_context(|| format!("reading /proc/{pid}/stat"))?;
    // The command name may hold spaces; the fields after it do not.
    let after_name = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest)
        .context("a /proc stat line")?;
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    let field = |at: usize| -> Result<u64> {
        fields
            .get(at)
            .context("a /proc stat line too short")?
            .parse()
            .context("a /proc stat CPU field")
    };
    Ok(field(11)? + field(12)?)
}
