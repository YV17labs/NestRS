use std::time::Duration;

use features::audio::AudioQueueModule;
use nest_rs::core::module;
use nest_rs::http::{StatusCode, header};
use nest_rs::queue::{QueueModule, QueueWorker};
use nest_rs::redis::{RedisModule, RedisQueueModule};
use nest_rs::testing::{HeadlessApp, LogCapture};
use nest_rs::testing::{TestForm, TestFormField};
use serde_json::{Value, json};

use crate::*;

fn quoted_in_a_log(logs: &LogCapture, text: &str) -> bool {
    logs.events()
        .iter()
        .any(|event| format!("{:?}", event.fields).contains(text))
}

#[tokio::test]
async fn audio_transcode_endpoint_enqueues_a_job_for_the_worker() {
    let (_db, app) = boot().await;
    let file = Uuid::now_v7().to_string();

    app.http()
        .post("/audio/transcode")
        .body_json(&json!({ "file": file }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let bearer = format!("Bearer {}", login().await);
    let logs = LogCapture::install();
    let resp = app
        .http()
        .post("/audio/transcode")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "file": file }))
        .send()
        .await;
    resp.assert_status(StatusCode::ACCEPTED);
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        body.is_empty() && !quoted_in_a_log(&logs, &file),
        "a key the client sends back reaches no reply or log line: {body:?} {:#?}",
        logs.events(),
    );

    app.http()
        .post("/audio/transcode")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "file": "track-1.mp3" }))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audio_transcode_rate_limit_answers_429_with_retry_after() {
    let (_db, app) = boot().await;
    let bearer = format!("Bearer {}", login().await);

    let file = Uuid::now_v7();
    for _ in 0..20 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": file }))
            .send()
            .await
            .assert_status(StatusCode::ACCEPTED);
    }
    let denied = app
        .http()
        .post("/audio/transcode")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "file": file }))
        .send()
        .await;
    denied.assert_status(StatusCode::TOO_MANY_REQUESTS);
    denied.assert_header_exist("retry-after");
}

#[module(
    imports = [
        RedisModule::for_root(None),
        RedisQueueModule,
        QueueModule::for_root(None),
        AudioQueueModule,
    ],
)]
struct AudioWorkerHarness;

#[tokio::test]
async fn audio_upload_transcode_and_result_round_trips_through_real_storage() {
    let (_db, app) = boot_on(RedisDatabase::AudioUpload).await;
    let http = reqwest::Client::new();

    let worker = HeadlessApp::builder()
        .module::<AudioWorkerHarness>()
        .provide(RedisDatabase::AudioUpload.config())
        .build_headless()
        .await
        .expect("the audio worker boots against Redis and storage");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the worker's QueueWorker drains the audio queue");

    let bearer = format!("Bearer {}", login().await);

    let ticket = app
        .http()
        .post("/audio/uploads")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    ticket.assert_status_is_ok();
    let ticket: Value = ticket.json().await.value().deserialize();
    let key = ticket["key"].as_str().expect("the minted key").to_owned();
    let put_url = ticket["url"].as_str().expect("the signed URL").to_owned();
    assert!(Uuid::parse_str(&key).is_ok(), "the key is minted: {key}");
    assert_eq!(ticket["method"], "PUT");
    assert_eq!(ticket["headers"], json!({ "content-type": "audio/mpeg" }));

    let html = http
        .put(&put_url)
        .header("content-type", "text/html")
        .body(b"<script>alert(1)</script>".to_vec())
        .send()
        .await
        .expect("PUT to the presigned upload URL");
    assert_eq!(
        html.status(),
        reqwest::StatusCode::FORBIDDEN,
        "the URL binds the audio type"
    );

    let payload = b"nestrs audio A4 e2e \xf0\x9f\x8e\xb5 payload".to_vec();
    let put = ticket["headers"]
        .as_object()
        .expect("the headers to send")
        .iter()
        .fold(http.put(&put_url), |put, (name, value)| {
            put.header(name, value.as_str().expect("a header value"))
        })
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
            .assert_status(StatusCode::ACCEPTED);

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
        .expect("the worker's QueueWorker stops cleanly");
}

#[tokio::test]
async fn audio_multipart_upload_and_streamed_download_round_trip() {
    let (_db, app) = boot_on(RedisDatabase::AudioMultipart).await;

    let worker = HeadlessApp::builder()
        .module::<AudioWorkerHarness>()
        .provide(RedisDatabase::AudioMultipart.config())
        .build_headless()
        .await
        .expect("the audio worker boots against Redis and storage");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the worker's QueueWorker drains the audio queue");

    let bearer = format!("Bearer {}", login().await);
    let filename = "../../escape.mp3";
    let payload = b"nestrs multipart + streaming payload \xf0\x9f\x8e\xa7".to_vec();

    let form = TestForm::new().field(
        TestFormField::bytes(payload.clone())
            .name("file")
            .filename(filename),
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
    assert!(
        Uuid::parse_str(&key).is_ok(),
        "the key is minted, never the part's filename: {key}"
    );

    let mut downloaded: Option<Vec<u8>> = None;
    'outer: for _ in 0..5 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": key }))
            .send()
            .await
            .assert_status(StatusCode::ACCEPTED);

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

    let never = Uuid::now_v7().to_string();
    let logs = LogCapture::install();
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
    assert!(
        !skipped.contains(&never) && !quoted_in_a_log(&logs, &never),
        "a poll for an unknown key replies and logs none of it: {skipped:?} {:#?}",
        logs.events(),
    );
    drop(logs);

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
        .expect("the worker's QueueWorker stops cleanly");
}
