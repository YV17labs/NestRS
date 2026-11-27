//! Covers `src/answer.rs` from the tick's side: what a trigger's method
//! answers, read by its type — an error of its own fails its occurrence as an
//! `anyhow::Error` does, its chain on the line saying so, through an alias or
//! a `macro_rules!` fragment as through the type written out — and a method
//! answering `!`, written out, through a fragment or under an alias, which
//! ends its occurrence as its body ends it; and a tick holding a large buffer
//! across an `.await`, which completes its occurrence on a worker's stack.

use std::any::TypeId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::operation_log::{ERROR, PANIC, TARGET as OPERATIONS};
use nest_rs_core::{Container, Transport};
use nest_rs_schedule::{ScheduledMethod, Scheduler, scheduled};
use nest_rs_testing::{LogCapture, wait_until};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
#[error("the report store refused the connection")]
struct StoreRefused;

#[derive(Debug, thiserror::Error)]
#[error("the nightly report failed")]
struct ReportFailed(#[source] StoreRefused);

/// What the sweep answers, under a name of its own.
type Swept = Result<(), StoreRefused>;

#[nest_rs_core::injectable]
#[derive(Default)]
struct ReportTasks;

#[scheduled]
impl ReportTasks {
    #[after("100ms")]
    async fn nightly(&self) -> Result<(), ReportFailed> {
        Err(ReportFailed(StoreRefused))
    }

    #[after("100ms")]
    fn sweep(&self) -> Swept {
        Err(StoreRefused)
    }
}

/// Every job of `H` the scheduler finds, run for half a second of paused time.
async fn tick<H: Default + Send + Sync + 'static>() {
    let container = Container::builder()
        .provide(nest_rs_core::__private::reachable_providers([
            TypeId::of::<H>(),
        ]))
        .provide(H::default())
        .build()
        .expect("the container builds");
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("the scheduler configures");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve joins")
        .expect("serve returns Ok");
}

#[tokio::test(start_paused = true)]
async fn an_error_of_its_own_fails_the_occurrence_with_its_whole_chain() {
    let logs = LogCapture::install();
    tick::<ReportTasks>().await;

    let failed = logs.find(nest_rs_schedule::TARGET, "scheduled job failed");
    let said = |method: &str| {
        failed
            .iter()
            .find(|line| line.field("method").as_deref() == Some(method))
            .unwrap_or_else(|| panic!("`{method}` failed unsaid: {failed:#?}"))
    };
    let nightly = said("nightly");
    assert_eq!(nightly.level, "error");
    assert_eq!(
        nightly.field("error").as_deref(),
        Some("the nightly report failed: the report store refused the connection"),
    );
    assert_eq!(
        said("sweep").field("error").as_deref(),
        Some("the report store refused the connection"),
        "an alias is read as the type it names, a `fn` as an `async fn`",
    );
    let outcomes: Vec<_> = logs
        .find(OPERATIONS, nest_rs_schedule::unit::TICK.name())
        .iter()
        .map(|line| line.field("outcome"))
        .collect();
    assert_eq!(outcomes, [Some(ERROR.to_owned()), Some(ERROR.to_owned())]);
}

// The answer comes through `tt` fragments, keeping the caller's spans, and
// `#[scheduled]` the macro's: what the expansion binds resolves in the answer's.
macro_rules! declare_fragment_tasks {
    ($($answer:tt)*) => {
        #[nest_rs_core::injectable]
        #[derive(Default)]
        struct FragmentTasks;

        #[scheduled]
        impl FragmentTasks {
            #[after("100ms")]
            async fn swept(&self) -> $($answer)* {
                Err(StoreRefused)
            }
        }
    };
}

declare_fragment_tasks!(Result<(), StoreRefused>);

#[tokio::test(start_paused = true)]
async fn an_answer_a_macro_fragment_writes_is_read_as_the_one_written_out() {
    assert!(
        nest_rs_core::inventory::iter::<ScheduledMethod>()
            .any(|job| job.host_type_id() == TypeId::of::<FragmentTasks>()),
        "the fragment's tick is submitted",
    );
    let logs = LogCapture::install();
    tick::<FragmentTasks>().await;

    let failed = logs.expect_one(nest_rs_schedule::TARGET, "scheduled job failed");
    assert_eq!(failed.field("method").as_deref(), Some("swept"));
    assert_eq!(
        failed.field("error").as_deref(),
        Some("the report store refused the connection"),
    );
}

/// `!` under a name, through the projection a toolchain before Rust 1.100 named
/// it by, which an app written then still carries.
type Never = <fn() -> ! as Returns>::Output;

trait Returns {
    type Output;
}

impl<T> Returns for fn() -> T {
    type Output = T;
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct DivergingTasks;

#[scheduled]
impl DivergingTasks {
    #[after("100ms")]
    fn drain(&self) -> Never {
        panic!("the drain is not built yet")
    }

    #[after("100ms")]
    fn halt(&self) -> ! {
        panic!("the halt is not built yet")
    }

    #[after("100ms", timeout = "100ms")]
    async fn pump(&self) -> ! {
        std::future::pending().await
    }
}

// A `ty` fragment wraps the `!` in an invisible group.
macro_rules! declare_fragment_diverging_tasks {
    ($answer:ty) => {
        #[nest_rs_core::injectable]
        #[derive(Default)]
        struct FragmentDivergingTasks;

        #[scheduled]
        impl FragmentDivergingTasks {
            #[after("100ms")]
            fn halt(&self) -> $answer {
                panic!("the halt is not built yet")
            }
        }
    };
}

declare_fragment_diverging_tasks!(!);

#[tokio::test(start_paused = true)]
async fn a_diverging_tick_ends_its_occurrence_as_its_body_does() {
    let logs = LogCapture::install();
    tick::<DivergingTasks>().await;
    tick::<FragmentDivergingTasks>().await;

    let mut ended: Vec<_> = logs
        .find(OPERATIONS, nest_rs_schedule::unit::TICK.name())
        .iter()
        .map(|line| {
            (
                line.field("host"),
                line.field("method"),
                line.field("outcome"),
            )
        })
        .collect();
    ended.sort();
    let ended_as = |host: &str, method: &str, outcome: &str| {
        (
            Some(host.to_owned()),
            Some(method.to_owned()),
            Some(outcome.to_owned()),
        )
    };
    assert_eq!(
        ended,
        [
            ended_as("DivergingTasks", "drain", PANIC),
            ended_as("DivergingTasks", "halt", PANIC),
            ended_as("DivergingTasks", "pump", ERROR),
            ended_as("FragmentDivergingTasks", "halt", PANIC),
        ],
        "a panic is contained and a wait cut at the tick's timeout, as the published release ran them",
    );
}

/// tokio's default worker stack, which `#[nest_rs::main]`'s runtime keeps.
const WORKER_STACK: usize = 2 * 1024 * 1024;

/// What `archive` holds across its `.await`: a third of its worker's stack.
const HELD: usize = WORKER_STACK / 3;

/// Set once `archive` ran past its `.await` to its end.
static ARCHIVED: AtomicBool = AtomicBool::new(false);

#[nest_rs_core::injectable]
#[derive(Default)]
struct ArchiveTasks;

#[scheduled]
impl ArchiveTasks {
    #[after("1ms")]
    async fn archive(&self) -> anyhow::Result<()> {
        let buffer = [7_u8; HELD];
        tokio::task::yield_now().await;
        std::hint::black_box(&buffer);
        ARCHIVED.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// A stack overflow aborts the process: nextest, which runs each test in a
/// process of its own, reports this one failed on `SIGABRT` and runs the rest.
#[test]
fn a_tick_holding_a_large_buffer_across_an_await_completes_its_occurrence() {
    // A runtime of the test's own, for its workers' stack, on the real clock:
    // tokio pauses its clock on a current-thread runtime alone.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("the runtime builds");
    runtime.block_on(async {
        let container = Container::builder()
            .provide(nest_rs_core::__private::reachable_providers([
                TypeId::of::<ArchiveTasks>(),
            ]))
            .provide(ArchiveTasks)
            .build()
            .expect("the container builds");
        let mut scheduler = Scheduler::new();
        scheduler
            .configure(&container)
            .await
            .expect("the scheduler configures");
        let cancel = CancellationToken::new();
        let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
        wait_until(Duration::from_secs(10), || ARCHIVED.load(Ordering::SeqCst)).await;
        cancel.cancel();
        serving
            .await
            .expect("serve joins")
            .expect("serve returns Ok");
    });
}
