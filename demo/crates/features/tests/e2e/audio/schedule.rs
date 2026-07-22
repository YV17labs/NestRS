use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use features::audio::{AudioQueue, AudioScheduleModule, TranscodeCommand};
use nest_rs_core::{injectable, module};
use nest_rs_queue::processor;
use nest_rs_redis::{QueueModule, QueueWorker, QueueWorkerModule};
use nest_rs_schedule::{ScheduleModule, Scheduler};
use nest_rs_testing::TestApp;

static SCHEDULED_FIRES: AtomicUsize = AtomicUsize::new(0);

fn started_ms() -> u128 {
    static STARTED: OnceLock<u128> = OnceLock::new();
    *STARTED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis()
    })
}

#[injectable]
#[derive(Default)]
pub struct CountingProcessor;

#[processor]
impl CountingProcessor {
    #[process(queue = AudioQueue)]
    async fn count(&self, job: TranscodeCommand) -> Result<()> {
        if let Some(ms) = job
            .file
            .strip_prefix("track-")
            .and_then(|rest| rest.strip_suffix(".mp3"))
            .and_then(|stem| stem.parse::<u128>().ok())
            && ms >= started_ms()
        {
            SCHEDULED_FIRES.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[module(
    imports = [QueueModule::for_root(None), ScheduleModule, AudioScheduleModule],
)]
struct ScheduleHarness;

#[module(
    imports = [QueueModule::for_root(None), QueueWorkerModule],
    providers = [CountingProcessor],
)]
struct CountingWorkerHarness;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_every_5s_audio_task_fires_and_lands_on_the_queue() {
    let _ = started_ms();

    let worker = TestApp::builder()
        .module::<CountingWorkerHarness>()
        .build_headless()
        .await
        .expect("the counting worker boots against Redis");
    let worker_handle = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the QueueWorker drains the audio queue");

    let schedule = TestApp::builder()
        .module::<ScheduleHarness>()
        .build_headless()
        .await
        .expect("the schedule harness boots against Redis + storage");
    let schedule_handle = schedule
        .spawn_transport(Scheduler::new())
        .await
        .expect("the Scheduler transport serves the discovered tasks");

    let deadline = Duration::from_secs(20);
    let poll = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    while SCHEDULED_FIRES.load(Ordering::SeqCst) == 0 && waited < deadline {
        tokio::time::sleep(poll).await;
        waited += poll;
    }

    let _ = schedule_handle.shutdown().await;
    let _ = worker_handle.shutdown().await;

    assert!(
        SCHEDULED_FIRES.load(Ordering::SeqCst) >= 1,
        "the #[every(\"5s\")] task must fire and its command must be consumed \
         from the queue within {deadline:?}",
    );
}
