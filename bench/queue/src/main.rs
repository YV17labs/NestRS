//! The nestrs job queue over Redis, measured through its public API alone:
//! drain, latency, push throughput and idle cost. Every run flushes the Redis
//! database the app is configured with, so point it at a throwaway one.

mod admin;
mod command;
mod fleet;
mod lane;
mod measure;
mod module;
mod probe;
mod processor;
mod replica;
mod table;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use nest_rs::core::{App, EnvPrefix};
use nest_rs::queue::JobProducer;
use nest_rs::redis::RedisConfig;

use crate::admin::Admin;
use crate::lane::Lane;
use crate::module::BenchModule;

const USAGE: &str = "\
usage: queue-bench <measurement> [--flag value]...

  drain    [--queue c16] [--jobs 5000] [--replicas 1]   push first, then time the replicas
  latency  [--queue c16] [--jobs 2000] [--rate 200]     push at a steady rate to an idle worker
  push     [--queue c16] [--jobs 5000] [--pushers 1]    push throughput, no worker
  idle     [--secs 30]                                  what an idle worker costs

  every measurement: [--runs 3] [--log warn] (the replicas' log filter)
  Redis is the app's own (<PREFIX>_REDIS__URL), flushed before every run.";

/// What every measurement shares: Redis, a producer, and its flags.
pub struct Bench {
    pub admin: Admin,
    pub producer: Arc<dyn JobProducer>,
    pub log: String,
    pub runs: usize,
}

#[nest_rs::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let measurement = args.next().unwrap_or_default();
    if measurement == "replica" {
        return replica::serve().await;
    }
    let mut flags = Flags::parse(args)?;
    let bench = Bench::open(&mut flags).await?;
    let report = match measurement.as_str() {
        "drain" => {
            let (lane, jobs, replicas) = (
                flags.lane()?,
                flags.take("jobs", 5000)?,
                flags.take("replicas", 1)?,
            );
            flags.done()?;
            measure::drain(&bench, lane, jobs, replicas).await?
        }
        "latency" => {
            let (lane, jobs, rate) = (
                flags.lane()?,
                flags.take("jobs", 2000)?,
                flags.take("rate", 200)?,
            );
            flags.done()?;
            measure::latency(&bench, lane, jobs, rate).await?
        }
        "push" => {
            let (lane, jobs, pushers) = (
                flags.lane()?,
                flags.take("jobs", 5000)?,
                flags.take("pushers", 1)?,
            );
            flags.done()?;
            measure::push(&bench, lane, jobs, pushers).await?
        }
        "idle" => {
            let secs = flags.take("secs", 30)?;
            flags.done()?;
            measure::idle(&bench, secs).await?
        }
        _ => bail!("{USAGE}"),
    };
    bench.admin.flush().await?;
    println!(
        "{}\n_Redis {}_",
        report.render(),
        bench.admin.version().await?
    );
    Ok(())
}

impl Bench {
    async fn open(flags: &mut Flags) -> Result<Self> {
        let runs = flags.take("runs", 3)?;
        let log = flags.take_text("log", "warn");
        let app = App::builder().module::<BenchModule>().build().await?;
        let config = app
            .container()
            .get::<RedisConfig>()
            .context("RedisModule resolves its config")?;
        if config.url == RedisConfig::default().url {
            bail!(
                "set {} to a throwaway Redis: the bench flushes it before every run",
                EnvPrefix::var("REDIS__URL")
            );
        }
        let producer = app
            .container()
            .get_dyn::<dyn JobProducer>()
            .context("RedisQueueModule binds the producer")?;
        Ok(Self {
            admin: Admin::from_url(&config.url)?,
            producer,
            log,
            runs,
        })
    }
}

struct Flags(HashMap<String, String>);

impl Flags {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut flags = HashMap::new();
        while let Some(flag) = args.next() {
            let Some(name) = flag.strip_prefix("--") else {
                bail!("`{flag}` is not a flag\n\n{USAGE}");
            };
            let value = args
                .next()
                .with_context(|| format!("--{name} takes a value"))?;
            flags.insert(name.to_string(), value);
        }
        Ok(Self(flags))
    }

    fn take<T: std::str::FromStr>(&mut self, name: &str, default: T) -> Result<T> {
        match self.0.remove(name) {
            None => Ok(default),
            Some(raw) => raw
                .parse()
                .map_err(|_| anyhow::anyhow!("--{name} takes a number, not `{raw}`")),
        }
    }

    fn take_text(&mut self, name: &str, default: &str) -> String {
        self.0.remove(name).unwrap_or_else(|| default.to_string())
    }

    fn lane(&mut self) -> Result<Lane> {
        Lane::parse(&self.take_text("queue", "c16"))
    }

    fn done(&self) -> Result<()> {
        match self.0.keys().next() {
            Some(name) => bail!("this measurement takes no --{name}\n\n{USAGE}"),
            None => Ok(()),
        }
    }
}
