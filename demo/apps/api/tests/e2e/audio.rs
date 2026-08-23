use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use features::audio::AudioQueueModule;
use nest_rs::config::Config;
use nest_rs::core::module;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisWorker, RedisWorkerModule};
use nest_rs::storage::{Storage, StorageConfig};
use nest_rs::testing::TestApp;
use poem::http::{StatusCode, header};
use poem::test::{TestForm, TestFormField};
use serde_json::json;

use crate::*;

#[tokio::test]
async fn audio_transcode_endpoint_enqueues_a_job_for_the_worker() {
    let (_db, app) = boot().await;

    app.http()
        .post("/audio/transcode")
        .body_json(&json!({ "file": "track-1.mp3" }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let bearer = format!("Bearer {}", login().await);
    let resp = app
        .http()
        .post("/audio/transcode")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "file": "track-1.mp3" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    assert_eq!(
        resp.json().await.value().object().get("file").string(),
        "track-1.mp3",
    );
}

#[tokio::test]
async fn audio_transcode_rate_limit_answers_429_with_retry_after() {
    let (_db, app) = boot().await;
    let bearer = format!("Bearer {}", login().await);

    for _ in 0..20 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": "track-throttle.mp3" }))
            .send()
            .await
            .assert_status_is_ok();
    }
    let denied = app
        .http()
        .post("/audio/transcode")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "file": "track-throttle.mp3" }))
        .send()
        .await;
    denied.assert_status(StatusCode::TOO_MANY_REQUESTS);
    denied.assert_header_exist("retry-after");
}

#[module(
    imports = [
        RedisModule::for_root(None),
        RedisQueueModule,
        RedisWorkerModule::for_root(None),
        AudioQueueModule,
    ],
)]
struct AudioWorkerHarness;

pub(crate) fn storage_client() -> Storage {
    let config = StorageConfig::load().expect("storage config parses from env");
    Storage::new(Arc::new(config))
}

async fn ensure_bucket(http: &reqwest::Client) {
    if let Ok(url) = storage_client()
        .presign_put("", Duration::from_secs(60))
        .await
    {
        let _ = http.put(&url).send().await;
    }
}

#[tokio::test]
async fn audio_upload_transcode_and_result_round_trips_through_real_storage() {
    let (_db, app) = boot().await;
    let http = reqwest::Client::new();
    ensure_bucket(&http).await;

    let worker = TestApp::builder()
        .module::<AudioWorkerHarness>()
        .build_headless()
        .await
        .expect("the audio worker boots against Redis and storage");
    let worker_queue = worker
        .spawn_transport(RedisWorker::new())
        .await
        .expect("the worker's RedisWorker drains the audio queue");

    let bearer = format!("Bearer {}", login().await);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let filename = format!("e2e-{}-{}.mp3", std::process::id(), nonce);

    let ticket = app
        .http()
        .post("/audio/uploads")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "filename": filename }))
        .send()
        .await;
    ticket.assert_status_is_ok();
    let ticket = ticket.json().await;
    let key = ticket.value().object().get("key").string().to_owned();
    let put_url = ticket.value().object().get("url").string().to_owned();

    let payload = b"nestrs audio A4 e2e \xf0\x9f\x8e\xb5 payload".to_vec();
    let put = http
        .put(&put_url)
        .body(payload.clone())
        .send()
        .await
        .expect("PUT to the presigned upload URL");
    assert!(
        put.status().is_success(),
        "presigned PUT failed: {} — {}",
        put.status(),
        put.text().await.unwrap_or_default(),
    );

    let mut result_url: Option<String> = None;
    'outer: for _ in 0..5 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": key }))
            .send()
            .await
            .assert_status_is_ok();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let resp = app
                .http()
                .get(format!("/audio/results?file={key}"))
                .header(header::AUTHORIZATION, &bearer)
                .send()
                .await;
            if resp.0.status() == StatusCode::OK {
                result_url = Some(
                    resp.json()
                        .await
                        .value()
                        .object()
                        .get("url")
                        .string()
                        .to_owned(),
                );
                break 'outer;
            }
        }
    }
    let result_url =
        result_url.expect("the worker produced the derived object and /audio/results served a URL");

    let got = http
        .get(&result_url)
        .send()
        .await
        .expect("GET the presigned result URL");
    assert!(
        got.status().is_success(),
        "presigned GET failed: {}",
        got.status(),
    );
    let got_bytes = got.bytes().await.expect("result body").to_vec();
    assert_eq!(
        got_bytes, payload,
        "the derived object's bytes match the uploaded payload",
    );

    worker_queue
        .shutdown()
        .await
        .expect("the worker's RedisWorker stops cleanly");
}

#[tokio::test]
async fn audio_multipart_upload_and_streamed_download_round_trip() {
    let (_db, app) = boot().await;
    let http = reqwest::Client::new();
    ensure_bucket(&http).await;

    let worker = TestApp::builder()
        .module::<AudioWorkerHarness>()
        .build_headless()
        .await
        .expect("the audio worker boots against Redis and storage");
    let worker_queue = worker
        .spawn_transport(RedisWorker::new())
        .await
        .expect("the worker's RedisWorker drains the audio queue");

    let bearer = format!("Bearer {}", login().await);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let filename = format!("e2e-multipart-{}-{}.mp3", std::process::id(), nonce);
    let payload = b"nestrs multipart + streaming payload \xf0\x9f\x8e\xa7".to_vec();

    let form = TestForm::new().field(
        TestFormField::bytes(payload.clone())
            .name("file")
            .filename(&filename),
    );
    let up = app
        .http()
        .post("/audio/uploads/direct")
        .header(header::AUTHORIZATION, &bearer)
        .multipart(form)
        .send()
        .await;
    up.assert_status_is_ok();
    let key = up
        .json()
        .await
        .value()
        .object()
        .get("key")
        .string()
        .to_owned();

    let mut downloaded: Option<Vec<u8>> = None;
    'outer: for _ in 0..5 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": key }))
            .send()
            .await
            .assert_status_is_ok();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let resp = app
                .http()
                .get(format!("/audio/download?file={key}"))
                .header(header::AUTHORIZATION, &bearer)
                .send()
                .await;
            if resp.0.status() == StatusCode::OK {
                let bytes = resp.0.into_body().into_bytes().await.expect("stream body");
                downloaded = Some(bytes.to_vec());
                break 'outer;
            }
        }
    }
    let downloaded = downloaded.expect("the streamed download served the derived object");
    assert_eq!(
        downloaded, payload,
        "the streamed bytes match the multipart upload",
    );

    let events = app
        .http()
        .get(format!("/audio/events?file={key}"))
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    events.assert_status_is_ok();
    events.assert_content_type("text/event-stream");
    let body = events.0.into_body().into_bytes().await.expect("sse body");
    let body = String::from_utf8(body.to_vec()).expect("sse is utf-8");
    assert!(
        body.contains("event: transcode") && body.contains("\"state\":\"ready\""),
        "the SSE feed emits a ready transcode event: {body:?}",
    );
    assert!(
        body.contains("id: 0"),
        "each event carries its poll attempt as the SSE id — what a browser \
         echoes back as Last-Event-ID: {body:?}",
    );

    let resumed = app
        .http()
        .get(format!("/audio/events?file={key}"))
        .header(header::AUTHORIZATION, &bearer)
        .header("Last-Event-ID", "9")
        .send()
        .await;
    resumed.assert_status_is_ok();
    let resumed = resumed.0.into_body().into_bytes().await.expect("sse body");
    let resumed = String::from_utf8(resumed.to_vec()).expect("sse is utf-8");
    assert!(
        resumed.contains("\"state\":\"ready\""),
        "a resumed stream still delivers the terminal state, whatever the \
         reconnecting client last saw: {resumed:?}",
    );

    let never = format!("absent-{}-{}.mp3", std::process::id(), nonce);
    let skipped = app
        .http()
        .get(format!("/audio/events?file={never}"))
        .header(header::AUTHORIZATION, &bearer)
        .header("Last-Event-ID", "5")
        .send()
        .await;
    skipped.assert_status_is_ok();
    let skipped = skipped.0.into_body().into_bytes().await.expect("sse body");
    let skipped = String::from_utf8(skipped.to_vec()).expect("sse is utf-8");
    assert!(
        !skipped.contains("id: 0") && skipped.contains("id: 6"),
        "the progress ticks the client already has are not replayed: {skipped:?}",
    );

    let malformed = app
        .http()
        .get(format!("/audio/events?file={key}"))
        .header(header::AUTHORIZATION, &bearer)
        .header("Last-Event-ID", "soon")
        .send()
        .await;
    malformed.assert_status(StatusCode::BAD_REQUEST);
    let malformed = malformed.0.into_body().into_bytes().await.expect("body");
    let malformed = String::from_utf8(malformed.to_vec()).expect("utf-8");
    assert!(
        malformed.contains("Last-Event-ID") && !malformed.contains("soon"),
        "a header rejection names the header and never echoes its value: {malformed:?}",
    );

    worker_queue
        .shutdown()
        .await
        .expect("the worker's RedisWorker stops cleanly");
}
