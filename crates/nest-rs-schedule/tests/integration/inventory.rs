//! What a program reads of a `#[scheduled]` method, and the defaults a job
//! attached by hand starts from.

use std::any::TypeId;
use std::time::Duration;

use nest_rs_core::{JOB_TIMEOUT, injectable};
use nest_rs_database::TransactionPolicy;
use nest_rs_schedule::{CronJobMeta, Replicas, RunFn, ScheduledMethod, Trigger, scheduled};

#[injectable]
#[derive(Default)]
struct LedgerSweeps;

#[scheduled]
impl LedgerSweeps {
    #[every("90s", replicas = "one", key = "ledger::sweep", timeout = "20s")]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cron("0 0 3 * * *", tz = "Europe/Paris")]
    async fn close(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

const NOTHING: RunFn = |_container| Box::pin(async { Ok(()) });

fn declared(method: &str) -> &'static ScheduledMethod {
    nest_rs_core::inventory::iter::<ScheduledMethod>()
        .find(|entry| {
            entry.provider_type_id() == TypeId::of::<LedgerSweeps>() && entry.method() == method
        })
        .expect("`#[scheduled]` submitted the method")
}

#[test]
fn a_scheduled_method_reads_back_what_its_decorator_declared() {
    let sweep = declared("sweep");
    assert!(
        sweep.origin().ends_with("::inventory"),
        "{}",
        sweep.origin()
    );
    assert_eq!(sweep.replicas(), Replicas::One);
    assert_eq!(sweep.key(), Some("ledger::sweep"));
    assert_eq!(sweep.timeout(), Duration::from_secs(20));
    assert_eq!(sweep.transaction(), TransactionPolicy::PerUnit);
    assert!(
        matches!(sweep.trigger(), Trigger::Interval(every) if every == Duration::from_secs(90)),
        "the period is read off `#[every]`"
    );

    let close = declared("close");
    assert!(matches!(
        close.trigger(),
        Trigger::Cron {
            expr: "0 0 3 * * *",
            tz: Some("Europe/Paris")
        }
    ));
}

#[test]
fn a_job_built_by_hand_starts_where_a_trigger_without_keys_does() {
    let close = declared("close");
    let by_hand = CronJobMeta::new(
        module_path!(),
        "LedgerSweeps",
        "close",
        close.trigger(),
        NOTHING,
    );

    assert_eq!(by_hand.timeout(), JOB_TIMEOUT);
    assert_eq!(by_hand.timeout(), close.timeout());
    assert_eq!(by_hand.transaction(), close.transaction());
    assert_eq!(by_hand.replicas(), close.replicas());
    assert_eq!(by_hand.key(), close.key());
    assert_eq!(
        (by_hand.origin(), by_hand.provider(), by_hand.method()),
        (module_path!(), "LedgerSweeps", "close")
    );
}

#[test]
fn each_key_a_job_built_by_hand_sets_reads_back() {
    let job = CronJobMeta::new(
        module_path!(),
        "LedgerSweeps",
        "sweep",
        Trigger::Interval(Duration::from_secs(90)),
        NOTHING,
    )
    .with_timeout(Duration::from_secs(20))
    .with_transaction(TransactionPolicy::Pool)
    .with_replicas(Replicas::One)
    .with_key("ledger::sweep");

    assert_eq!(job.timeout(), Duration::from_secs(20));
    assert_eq!(job.transaction(), TransactionPolicy::Pool);
    assert_eq!(job.replicas(), Replicas::One);
    assert_eq!(job.key(), Some("ledger::sweep"));
}
