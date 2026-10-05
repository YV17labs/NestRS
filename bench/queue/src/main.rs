//! The nestrs job queue over Redis, measured through its public API alone:
//! drain, latency, push throughput and idle cost. Every run deletes the keys the
//! bench's queues left, and the bench refuses a Redis holding anything else.

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
use nest_rs::core::App;
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
  Redis is the app's own (<PREFIX>_REDIS__URL), and holds nothing but the bench's
  keys: the bench deletes them before every run and refuses a Redis holding more.";

/// What every measurement shares: Redis, a producer, and its flags.
pub struct Bench {
    pub admin: Admin,
    pub producer: Arc<dyn JobProducer>,
    pub log: String,
    pub runs: usize,
}

enum Measurement {
    Drain {
        lane: Lane,
        jobs: u32,
        replicas: usize,
    },
    Latency {
        lane: Lane,
        jobs: u32,
        rate: u32,
    },
    Push {
        lane: Lane,
        jobs: u32,
        pushers: u32,
    },
    Idle {
        secs: u64,
    },
}

#[nest_rs::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_default();
    let mut flags = Flags::parse(args)?;
    let measurement = match name.as_str() {
        "replica" => return replica::serve().await,
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            return Ok(());
        }
        "drain" => Measurement::Drain {
            lane: flags.lane()?,
            jobs: flags.take("jobs", 5000)?,
            replicas: flags.take("replicas", 1)?,
        },
        "latency" => Measurement::Latency {
            lane: flags.lane()?,
            jobs: flags.take("jobs", 2000)?,
            rate: flags.take("rate", 200)?,
        },
        "push" => Measurement::Push {
            lane: flags.lane()?,
            jobs: flags.take("jobs", 5000)?,
            pushers: flags.take("pushers", 1)?,
        },
        "idle" => Measurement::Idle {
            secs: flags.take("secs", 30)?,
        },
        _ => bail!("{USAGE}"),
    };
    let runs = flags.take("runs", 3)?;
    let log = flags.take_text("log", "warn");
    flags.done()?;

    let bench = Bench::open(runs, log).await?;
    let report = match measurement {
        Measurement::Drain {
            lane,
            jobs,
            replicas,
        } => measure::drain(&bench, lane, jobs, replicas).await?,
        Measurement::Latency { lane, jobs, rate } => {
            measure::latency(&bench, lane, jobs, rate).await?
        }
        Measurement::Push {
            lane,
            jobs,
            pushers,
        } => measure::push(&bench, lane, jobs, pushers).await?,
        Measurement::Idle { secs } => measure::idle(&bench, secs).await?,
    };
    bench.admin.clear().await?;
    println!(
        "{}\n_Redis {}_",
        report.render(),
        bench.admin.version().await?
    );
    Ok(())
}

impl Bench {
    async fn open(runs: usize, log: String) -> Result<Self> {
        let app = App::builder().module::<BenchModule>().build().await?;
        let config = app
            .container()
            .get::<RedisConfig>()
            .context("RedisModule resolves its config")?;
        let admin = Admin::from_url(&config.url)?;
        admin.claim().await?;
        let producer = app
            .container()
            .get_dyn::<dyn JobProducer>()
            .context("RedisQueueModule binds the producer")?;
        Ok(Self {
            admin,
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
