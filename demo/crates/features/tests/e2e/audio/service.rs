use std::sync::Arc;

use features::audio::{AudioModule, AudioService, TranscodeState, Transcodes};
use futures_util::StreamExt;
use nest_rs::core::module;
use nest_rs::redis::{RedisConnection, RedisModule, RedisQueueModule};
use nest_rs::storage::Storage;
use nest_rs::testing::HeadlessApp;
use uuid::Uuid;

#[module(imports = [RedisModule::for_root(None), RedisQueueModule, AudioModule])]
struct AudioHarness;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transcode_files_its_record_on_the_transcodes_instance_and_the_progress_reads_it() {
    let app = HeadlessApp::builder()
        .module::<AudioHarness>()
        .build_headless()
        .await
        .expect("the audio feature boots against both Redis instances and storage");
    let container = app.container();
    let svc = container
        .get::<AudioService>()
        .expect("AudioModule provides the service");
    let storage = container
        .get::<Storage>()
        .expect("AudioModule opens the store");
    let file = Uuid::now_v7();
    storage
        .put_bytes(&file.to_string(), "a source to transcode", "audio/mpeg")
        .await
        .expect("the source is stored");

    let mut progress = Box::pin(Arc::clone(&svc).transcode_events(file));
    let pending = progress.next().await.expect("a first progress event");
    assert!(
        matches!(pending.state, TranscodeState::Pending),
        "nothing is transcoded yet: {:?}",
        pending.state
    );

    svc.transcode(file).await.expect("the transcode runs");
    let ready = progress.next().await.expect("a next progress event");
    assert!(
        matches!(ready.state, TranscodeState::Ready) && ready.attempt > 0,
        "the record the transcode filed is what the next poll reads: {:?} at attempt {}",
        ready.state,
        ready.attempt
    );

    let recorded: bool = redis::cmd("EXISTS")
        .arg(format!("audio:transcoded:{file}"))
        .query_async(&mut RedisConnection::clone(
            &container
                .get::<RedisConnection<Transcodes>>()
                .expect("AudioModule opens the transcodes instance"),
        ))
        .await
        .expect("the transcodes instance answers");
    assert!(recorded, "the record lives on the transcodes instance");
}
