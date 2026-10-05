//! The kit's processors — one per case, each with one `#[process]` method on
//! its case's queue, declaring what the case proves, all doing what the job's
//! [`Act`] says.

use std::time::Duration;

use nest_rs_core::injectable;
use nest_rs_queue::{Queue, processor};

use super::command::{
    Act, BudgetQueue, ConcurrencyQueue, DeathQueue, DelayQueue, DrainQueue, KitCommand, OnceQueue,
    RenewalQueue, RetryQueue, StallQueue, TakenQueue, TraceQueue,
};
use super::probe;

/// One processor per case, each with the one method its case's queue needs,
/// declaring what the case proves.
macro_rules! processors {
    ($($processor:ident: $method:ident on $queue:ident $(, $key:ident = $value:literal)*;)+) => {
        $(
            #[doc = concat!("The processor of the case on `", stringify!($queue), "`.")]
            #[injectable]
            #[derive(Default)]
            pub(crate) struct $processor;

            #[processor]
            impl $processor {
                #[process(queue = $queue $(, $key = $value)*)]
                async fn $method(&self, job: KitCommand) -> anyhow::Result<()> {
                    act($queue::NAME, job).await
                }
            }
        )+
    };
}

processors! {
    OnceProcessor: once on OnceQueue;
    ConcurrencyProcessor: concurrency on ConcurrencyQueue, concurrency = 3;
    RetryProcessor: retry on RetryQueue, retries = 1;
    BudgetProcessor: budget on BudgetQueue, retries = 1;
    DeathProcessor: death on DeathQueue;
    TakenProcessor: taken on TakenQueue;
    StallProcessor: stall on StallQueue;
    DrainProcessor: drain on DrainQueue, concurrency = 2;
    RenewalProcessor: renewal on RenewalQueue;
    DelayProcessor: delay on DelayQueue;
    TraceProcessor: trace on TraceQueue;
}

/// Run one attempt at `job` on `queue` as its [`Act`] says, reporting it.
async fn act(queue: &'static str, job: KitCommand) -> anyhow::Result<()> {
    let (running, attempt) = probe::start(queue, &job);
    let outcome = match job.act {
        Act::Complete => Ok(()),
        Act::Hold { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        }
        Act::FailFirst { attempts } if attempt <= attempts => {
            Err(anyhow::anyhow!("the kit's job fails attempt {attempt}"))
        }
        Act::FailFirst { .. } => Ok(()),
        Act::FailAlways => Err(anyhow::anyhow!("the kit's job fails every attempt")),
        Act::ParkOnce if attempt == 1 => std::future::pending().await,
        Act::ParkOnce => Ok(()),
        Act::ParkAlways => std::future::pending().await,
    };
    running.end(outcome.is_ok());
    outcome
}
