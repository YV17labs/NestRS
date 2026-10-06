//! The kit's pieces: the backend seam a driver implements ([`KitBackend`]), the
//! processor every case's worker runs, the probe it reports to, and the cases.

mod backend;
pub mod cases;
mod command;
mod module;
mod probe;
mod processor;

pub use backend::KitBackend;

/// Run one case against the backend `backend` builds, on a runtime of its own —
/// paused when the backend lives in this process
/// ([`KitBackend::IN_PROCESS`]) — with the global log capture its assertions
/// read installed first.
pub fn run<B, F, Fut>(backend: impl FnOnce() -> B, case: F)
where
    B: KitBackend,
    F: FnOnce(B, crate::LogCapture) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let logs = crate::LogCapture::install_global();
    let runtime = if B::IN_PROCESS {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
    } else {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
    };
    runtime
        .expect("a runtime for the kit's case")
        .block_on(case(backend(), logs));
}

/// One `#[test]` per case of the kit, each against `$backend` — an expression
/// evaluated afresh in every test, building a [`KitBackend`].
///
/// ```
/// # use std::time::Duration;
/// # use nest_rs_queue::QueueName;
/// # use nest_rs_testing::queue::KitBackend;
/// # use nest_rs_testing::{TestApp, TestAppBuilder};
/// # struct MyBackend;
/// # impl MyBackend {
/// #     fn new() -> Self {
/// #         Self
/// #     }
/// # }
/// # impl KitBackend for MyBackend {
/// #     fn app(&self) -> TestAppBuilder {
/// #         TestApp::builder()
/// #     }
/// #     fn lease(&self) -> Duration {
/// #         Duration::from_secs(1)
/// #     }
/// #     async fn purge(&self, _: &QueueName) -> anyhow::Result<()> {
/// #         Ok(())
/// #     }
/// #     async fn lapse(&self, _: &QueueName) -> anyhow::Result<()> {
/// #         Ok(())
/// #     }
/// #     async fn take(&self, _: &QueueName) -> anyhow::Result<()> {
/// #         Ok(())
/// #     }
/// # }
/// nest_rs_testing::queue_kit!(MyBackend::new());
/// # fn main() {}
/// ```
#[macro_export]
macro_rules! queue_kit {
    ($backend:expr) => {
        $crate::queue_kit!(@cases $backend;
            a_pushed_job_runs_once,
            a_method_runs_no_more_attempts_at_once_than_its_concurrency,
            a_retryable_failure_runs_again_as_the_next_attempt,
            a_spent_budget_dead_letters_the_job,
            a_job_whose_worker_died_mid_attempt_runs_on_another,
            a_lease_lost_under_a_live_worker_cuts_its_attempt_and_the_holder_decides,
            a_job_taking_its_worker_down_past_the_stall_limit_is_dead_lettered_unrun,
            the_drain_finishes_what_fits_its_window_and_hands_back_the_rest,
            a_long_attempt_keeps_its_lease_and_runs_once,
            a_delayed_push_runs_once_its_delay_has_passed,
            a_job_runs_in_the_trace_that_pushed_it,
        );
    };
    (@cases $backend:expr; $($case:ident),+ $(,)?) => {
        $(
            #[test]
            fn $case() {
                $crate::queue::run(|| $backend, $crate::queue::cases::$case);
            }
        )+
    };
}
