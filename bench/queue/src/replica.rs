//! One worker replica, run as a child process of a measurement, and the
//! protocol it speaks with its parent on its standard streams.
//!
//! The replica prints `@ready` once its app is built, then every handler run as
//! `@run <seq> <pushed_us> <started_us> <done_us>`, batched every
//! [`REPORT_EVERY`]. Its standard input closing is the parent saying it has
//! what it came for: the replica reports what is left and exits without
//! draining — the next run flushes Redis anyway. Every other line on its
//! standard output is the framework's own log.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nest_rs::core::App;
use tokio::io::AsyncReadExt;

use crate::module::BenchModule;
use crate::probe::{Probe, Run};

pub const READY: &str = "@ready";
pub const RUN: &str = "@run ";

const REPORT_EVERY: Duration = Duration::from_millis(10);

pub async fn serve() -> Result<()> {
    let app = App::builder().module::<BenchModule>().build().await?;
    let probe = app
        .container()
        .get::<Probe>()
        .context("BenchModule provides the Probe")?;
    let mut serving = tokio::spawn(app.run());
    say(&format!("{READY}\n"))?;

    let mut parent_gone = tokio::spawn(async {
        let mut sink = Vec::new();
        tokio::io::stdin().read_to_end(&mut sink).await
    });
    let mut tick = tokio::time::interval(REPORT_EVERY);
    loop {
        tokio::select! {
            _ = tick.tick() => report(probe.take())?,
            _ = &mut parent_gone => break,
            stopped = &mut serving => match stopped? {
                Ok(()) => bail!("the worker stopped before its parent asked"),
                Err(error) => return Err(error.context("the worker failed")),
            },
        }
    }
    report(probe.take())?;
    std::process::exit(0)
}

fn report(runs: Vec<Run>) -> Result<()> {
    if runs.is_empty() {
        return Ok(());
    }
    let mut lines = String::new();
    for run in runs {
        writeln!(
            lines,
            "{RUN}{} {} {} {}",
            run.seq, run.pushed_us, run.started_us, run.done_us
        )?;
    }
    say(&lines)
}

/// Whole lines under the lock the framework's logger writes through too, so a
/// log line never lands inside a report.
fn say(lines: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(lines.as_bytes())?;
    out.flush()?;
    Ok(())
}

pub fn parse_run(line: &str) -> Option<Run> {
    let mut fields = line.strip_prefix(RUN)?.split(' ');
    let run = Run {
        seq: fields.next()?.parse().ok()?,
        pushed_us: fields.next()?.parse().ok()?,
        started_us: fields.next()?.parse().ok()?,
        done_us: fields.next()?.parse().ok()?,
    };
    fields.next().is_none().then_some(run)
}
