//! The four measurements. Each repeats its run `--runs` times on an emptied
//! Redis and returns one table row per run.

use std::time::Duration;

use anyhow::Result;

use crate::Bench;
use crate::admin::Counters;
use crate::fleet::{Fleet, Ledger};
use crate::lane::Lane;
use crate::probe::now_us;
use crate::table::{Table, breakdown, busiest, ms, percentile};

/// The longest one run may take: a drain at concurrency 1 runs for minutes.
const RUN_WITHIN: Duration = Duration::from_secs(30 * 60);

/// How long a replica runs idle before a measurement starts on it, past its
/// boot and its first sweeps.
const SETTLE: Duration = Duration::from_secs(5);

/// How the drain's backlog is filed; not what it measures.
const PREFILL_PUSHERS: u32 = 16;

const TOP: usize = 8;

/// `jobs` pushed first, then `replicas` started: from their spawn to the last
/// job's first completion.
pub async fn drain(bench: &Bench, lane: Lane, jobs: u32, replicas: usize) -> Result<Table> {
    let mut table = Table::new(
        format!(
            "Drain — {jobs} jobs on {}, {replicas} replica process(es)",
            lane.describe()
        ),
        &[
            "drain ms",
            "jobs/s",
            "first job ms",
            "worker CPU ms",
            "CPU µs/job",
            "Redis CPU ms",
            "Redis cmds/job",
            "duplicates",
        ],
    );
    for run in 1..=bench.runs {
        bench.admin.flush().await?;
        lane.push_all(&bench.producer, jobs, PREFILL_PUSHERS)
            .await?;
        let before = bench.admin.counters().await?;
        let spawned_us = now_us();
        let mut fleet = Fleet::spawn(replicas, &bench.log)?;
        let mut ledger = Ledger::new(jobs);
        fleet.until_complete(&mut ledger, RUN_WITHIN).await?;
        let worker_cpu = fleet.cpu()?;
        let after = bench.admin.counters().await?;
        fleet.stop(&mut ledger).await?;

        let done = |pick: fn(u64, u64) -> u64| ledger.runs().map(|r| r.done_us).reduce(pick);
        let drain = since(spawned_us, done(u64::max));
        let first = since(spawned_us, done(u64::min));
        let commands = busiest(&before.by_command, &after.by_command);
        let per_job = f64::from(jobs);
        table.run(vec![
            ms(drain),
            per_job / drain.as_secs_f64(),
            ms(first),
            ms(worker_cpu),
            ms(worker_cpu) * 1000.0 / per_job,
            ms(redis_cpu(&before, &after)),
            total(&commands) / per_job,
            ledger.duplicates as f64,
        ]);
        table.note(format!(
            "run {run}, Redis commands per job: {}",
            breakdown(&commands, per_job, "", TOP)
        ));
    }
    Ok(table)
}

/// An idle worker, then `jobs` pushed at `rate` a second: from just before each
/// push to its handler starting.
pub async fn latency(bench: &Bench, lane: Lane, jobs: u32, rate: u32) -> Result<Table> {
    let mut table = Table::new(
        format!(
            "Latency — push → handler start, {jobs} jobs at {rate}/s on {}, 1 idle replica",
            lane.describe()
        ),
        &["p50 ms", "p90 ms", "p99 ms", "max ms", "pushed/s"],
    );
    for _ in 0..bench.runs {
        bench.admin.flush().await?;
        let mut fleet = Fleet::spawn(1, &bench.log)?;
        let mut ledger = Ledger::new(jobs);
        fleet.ready(&mut ledger).await?;
        tokio::time::sleep(SETTLE).await;

        let mut tick = tokio::time::interval(Duration::from_secs(1) / rate.max(1));
        let started = tokio::time::Instant::now();
        for seq in 0..jobs {
            tick.tick().await;
            lane.push(&*bench.producer, seq).await?;
        }
        let pushing = started.elapsed();
        fleet.until_complete(&mut ledger, RUN_WITHIN).await?;
        fleet.stop(&mut ledger).await?;

        let waited: Vec<f64> = ledger
            .runs()
            .map(|r| r.started_us.saturating_sub(r.pushed_us) as f64 / 1000.0)
            .collect();
        table.run(vec![
            percentile(&waited, 50.0),
            percentile(&waited, 90.0),
            percentile(&waited, 99.0),
            percentile(&waited, 100.0),
            f64::from(jobs) / pushing.as_secs_f64(),
        ]);
    }
    Ok(table)
}

/// `jobs` single pushes from `pushers` concurrent tasks, no worker running.
pub async fn push(bench: &Bench, lane: Lane, jobs: u32, pushers: u32) -> Result<Table> {
    let mut table = Table::new(
        format!(
            "Push — {jobs} single pushes on {}, {pushers} concurrent pusher(s)",
            lane.describe()
        ),
        &["jobs/s", "elapsed ms", "Redis CPU ms", "Redis cmds/push"],
    );
    for run in 1..=bench.runs {
        bench.admin.flush().await?;
        let before = bench.admin.counters().await?;
        let elapsed = lane.push_all(&bench.producer, jobs, pushers).await?;
        let after = bench.admin.counters().await?;
        let commands = busiest(&before.by_command, &after.by_command);
        let per_push = f64::from(jobs);
        table.run(vec![
            per_push / elapsed.as_secs_f64(),
            ms(elapsed),
            ms(redis_cpu(&before, &after)),
            total(&commands) / per_push,
        ]);
        table.note(format!(
            "run {run}, Redis commands per push: {}",
            breakdown(&commands, per_push, "", TOP)
        ));
    }
    Ok(table)
}

/// One replica with nothing to do, past its boot, for `secs`.
pub async fn idle(bench: &Bench, secs: u64) -> Result<Table> {
    let mut table = Table::new(
        format!("Idle — 1 replica serving both queues, nothing to do, {secs} s"),
        &[
            "Redis cmds/s",
            "Redis CPU ms/s",
            "worker CPU ms",
            "worker CPU % of a core",
        ],
    );
    let window = Duration::from_secs(secs);
    for run in 1..=bench.runs {
        bench.admin.flush().await?;
        let mut fleet = Fleet::spawn(1, &bench.log)?;
        let mut ledger = Ledger::new(0);
        fleet.ready(&mut ledger).await?;
        tokio::time::sleep(SETTLE).await;
        let (cpu_before, before) = (fleet.cpu()?, bench.admin.counters().await?);
        tokio::time::sleep(window).await;
        let (cpu_after, after) = (fleet.cpu()?, bench.admin.counters().await?);
        fleet.stop(&mut ledger).await?;

        let commands = busiest(&before.by_command, &after.by_command);
        let worker = cpu_after.saturating_sub(cpu_before);
        table.run(vec![
            total(&commands) / window.as_secs_f64(),
            ms(redis_cpu(&before, &after)) / window.as_secs_f64(),
            ms(worker),
            worker.as_secs_f64() / window.as_secs_f64() * 100.0,
        ]);
        table.note(format!(
            "run {run}, Redis commands per second: {}",
            breakdown(&commands, window.as_secs_f64(), "/s", TOP)
        ));
    }
    Ok(table)
}

fn since(start_us: u64, end_us: Option<u64>) -> Duration {
    Duration::from_micros(end_us.unwrap_or(start_us).saturating_sub(start_us))
}

fn redis_cpu(before: &Counters, after: &Counters) -> Duration {
    after.cpu.saturating_sub(before.cpu)
}

fn total(commands: &[(String, u64)]) -> f64 {
    commands.iter().map(|(_, calls)| *calls as f64).sum()
}
