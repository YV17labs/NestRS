//! Presign, list and streamed-upload round-trips through [`Storage`]
//! (`src/client.rs`) against the live S3-compatible server.

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use futures_util::StreamExt;
use nest_rs_storage::{MULTIPART_PART_SIZE, StorageConfig, StorageError, TARGET};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

use crate::{Carry, ensure_bucket, proxied, proxy, storage, unique};

/// What the client logs about an interrupted upload's parts, copied: exported,
/// a wording change would break the API.
const DISCARDED: &str = "discarded the parts of an interrupted multipart upload";
const DANGLING: &str = "multipart upload left dangling parts";
const CANCELLED: &str = "multipart upload was cancelled mid-flight; discarding its parts";

/// The `nest_rs::storage` events a call emitted, as `(message, key)` — the
/// abort's only witness, since `object_store` cannot list multipart uploads.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<(String, String)>>>);

impl Events {
    /// The keys `message` was emitted for, in order.
    fn keys_for(&self, message: &str) -> Vec<String> {
        self.0
            .lock()
            .expect("events")
            .iter()
            .filter(|(emitted, _)| emitted == message)
            .map(|(_, key)| key.clone())
            .collect()
    }
}

#[derive(Default)]
struct Captured {
    message: String,
    key: String,
}

impl Visit for Captured {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "key" {
            self.key = value.to_owned();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `message` is a `fmt::Arguments`, whose `Debug` is its `Display`.
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Events {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut captured = Captured::default();
        event.record(&mut captured);
        self.0
            .lock()
            .expect("events")
            .push((captured.message, captured.key));
    }
}

#[tokio::test]
async fn presign_put_get_round_trip() {
    let s = storage();
    let http = reqwest::Client::new();
    ensure_bucket(&s, &http).await;

    let key = unique("hello.txt");
    let key = key.as_str();
    let body = b"object_store presign round-trip \xf0\x9f\x9a\x80".to_vec();

    let put_url = s
        .presign_put(key, Duration::from_secs(300))
        .await
        .expect("presign_put");
    let put_resp = http
        .put(&put_url)
        .header("content-type", "text/plain")
        .body(body.clone())
        .send()
        .await
        .expect("PUT send");
    assert!(
        put_resp.status().is_success(),
        "presigned PUT failed: {} — {}",
        put_resp.status(),
        put_resp.text().await.unwrap_or_default()
    );
    eprintln!("PUT  {key} -> 200");

    let get_url = s
        .presign_get(key, Duration::from_secs(300))
        .await
        .expect("presign_get");
    let got = http.get(&get_url).send().await.expect("GET send");
    assert!(
        got.status().is_success(),
        "presigned GET failed: {}",
        got.status()
    );
    let got_bytes = got.bytes().await.expect("GET body").to_vec();
    assert_eq!(got_bytes, body, "presigned GET bytes mismatch");
    eprintln!("GET(presigned) {} -> {} bytes match", key, got_bytes.len());

    let server_bytes = s.get_bytes(key).await.expect("get_bytes");
    assert_eq!(server_bytes.as_ref(), body.as_slice(), "get_bytes mismatch");
    eprintln!(
        "get_bytes      {} -> {} bytes match",
        key,
        server_bytes.len()
    );

    let info = s.head(key).await.expect("head").expect("object present");
    assert_eq!(info.byte_size, body.len() as i64, "head size mismatch");
    eprintln!("head           {} -> size={}", key, info.byte_size);

    let absent = s
        .head(&unique("does-not-exist"))
        .await
        .expect("head absent");
    assert!(absent.is_none(), "expected None for absent object");
    eprintln!("head(absent)   -> None (Ok)");

    let key2 = unique("variant.webp");
    let key2 = key2.as_str();
    s.put_bytes(key2, vec![1, 2, 3, 4], "image/webp")
        .await
        .expect("put_bytes");
    let rt = s.get_bytes(key2).await.expect("get_bytes key2");
    assert_eq!(rt.as_ref(), &[1, 2, 3, 4], "put_bytes round-trip mismatch");
    eprintln!("put_bytes/get  {} -> 4 bytes match", key2);

    s.delete(key).await.expect("delete key");
    s.delete(key2).await.expect("delete key2");
}

#[tokio::test]
async fn put_stream_uploads_in_parts_and_keeps_the_content_type() {
    let s = storage();
    let http = reqwest::Client::new();
    ensure_bucket(&s, &http).await;

    let key = unique("stream.mp3");
    // Past the 5 MiB part size, so the upload ships two parts.
    let chunk_size = 256 * 1024;
    let chunks: Vec<Vec<u8>> = (0..24u8)
        .map(|n| vec![n.wrapping_mul(7).wrapping_add(1); chunk_size])
        .collect();
    let expected: Vec<u8> = chunks.iter().flatten().copied().collect();
    assert!(expected.len() > 5 * 1024 * 1024, "payload spans two parts");

    let source = futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|chunk| Ok(bytes::Bytes::from(chunk))),
    );
    s.put_stream(&key, "audio/mpeg", source)
        .await
        .expect("put_stream");
    eprintln!("put_stream     {} -> {} bytes", key, expected.len());

    let got = s.get_bytes(&key).await.expect("get_bytes after put_stream");
    assert_eq!(got.len(), expected.len(), "streamed upload size mismatch");
    assert!(got.as_ref() == expected, "streamed upload bytes mismatch");

    let info = s.head(&key).await.expect("head").expect("object present");
    assert_eq!(info.byte_size, expected.len() as i64, "head size mismatch");

    // `head` cannot report the content type, so it is read off the wire.
    let get_url = s
        .presign_get(&key, Duration::from_secs(300))
        .await
        .expect("presign_get");
    let served = http.get(&get_url).send().await.expect("GET send");
    assert!(
        served.status().is_success(),
        "GET failed: {}",
        served.status()
    );
    let content_type = served
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(content_type, "audio/mpeg", "content type did not survive");
    eprintln!("GET(presigned) {key} -> {content_type}");

    let empty_key = unique("stream-empty.mp3");
    s.put_stream(&empty_key, "audio/mpeg", futures_util::stream::empty())
        .await
        .expect("put_stream of an empty source");
    let empty = s
        .head(&empty_key)
        .await
        .expect("head")
        .expect("empty object present");
    assert_eq!(empty.byte_size, 0, "an empty stream stores zero bytes");
    eprintln!("put_stream     {empty_key} -> 0 bytes");

    s.delete(&key).await.expect("delete");
    s.delete(&empty_key).await.expect("delete empty");
}

#[tokio::test]
async fn a_failing_source_surfaces_its_own_error_and_leaves_no_object_or_parts_behind() {
    let s = storage();
    ensure_bucket(&s, &reqwest::Client::new()).await;

    let prefix = unique("aborted");
    let key = format!("{prefix}/interrupted.mp3");

    const SOURCE_FAILURE: &str = "the reader went away mid-upload";
    let chunk = vec![7u8; 512 * 1024];
    // Past one part, so the abort has parts to discard.
    let full_parts = MULTIPART_PART_SIZE / chunk.len() + 2;
    let source = futures_util::stream::iter(
        (0..full_parts).map(move |_| Ok(bytes::Bytes::from(chunk.clone()))),
    )
    .chain(futures_util::stream::once(async {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            SOURCE_FAILURE,
        ))
    }));

    let events = Events::default();
    let failed = {
        let _capture = tracing_subscriber::registry()
            .with(events.clone())
            .set_default();
        s.put_stream(&key, "audio/mpeg", source).await
    };

    let err = failed.expect_err("a source that fails mid-upload cannot report success");
    match &err {
        StorageError::PutSource(source) => {
            assert_eq!(source.kind(), std::io::ErrorKind::BrokenPipe);
            assert!(
                source.to_string().contains(SOURCE_FAILURE),
                "the source's error is handed back verbatim: {source}",
            );
        }
        other => panic!("expected PutSource, got {other:?}"),
    }

    assert!(
        s.head(&key).await.expect("head").is_none(),
        "an interrupted upload must not materialize an object",
    );
    let mut listing = std::pin::pin!(s.list(&prefix).expect("list"));
    assert!(
        listing.next().await.is_none(),
        "and nothing is listed under {prefix}",
    );

    assert_eq!(
        events.keys_for(DISCARDED),
        vec![key.clone()],
        "the interrupted upload's parts were not aborted",
    );
    assert!(
        events.keys_for(DANGLING).is_empty(),
        "the abort itself failed: {:?}",
        events.keys_for(DANGLING),
    );
    eprintln!("put_stream(failing source) {key} -> {err}, parts discarded");
}

#[tokio::test]
async fn a_successful_streamed_upload_discards_nothing() {
    let s = storage();
    ensure_bucket(&s, &reqwest::Client::new()).await;

    let key = unique("completed.mp3");
    let chunk = vec![3u8; 512 * 1024];
    let full_parts = MULTIPART_PART_SIZE / chunk.len() + 2;
    let expected = chunk.len() * full_parts;
    let source = futures_util::stream::iter(
        (0..full_parts).map(move |_| Ok(bytes::Bytes::from(chunk.clone()))),
    );

    let events = Events::default();
    {
        let _capture = tracing_subscriber::registry()
            .with(events.clone())
            .set_default();
        s.put_stream(&key, "audio/mpeg", source)
            .await
            .expect("put_stream");
    }

    let info = s.head(&key).await.expect("head").expect("object present");
    assert_eq!(info.byte_size, expected as i64);
    assert!(
        events.keys_for(DISCARDED).is_empty() && events.keys_for(DANGLING).is_empty(),
        "a completed upload aborted nothing",
    );

    s.delete(&key).await.expect("delete");
}

#[tokio::test]
async fn list_streams_exactly_the_objects_under_a_prefix() {
    let s = storage();
    ensure_bucket(&s, &reqwest::Client::new()).await;

    let prefix = unique("listing");
    let inside = [
        (format!("{prefix}/a.txt"), vec![1u8, 2, 3]),
        (format!("{prefix}/nested/b.bin"), vec![4u8; 7]),
    ];
    // Shares the prefix's characters but not its path segments.
    let outside = format!("{prefix}-other/c.txt");
    for (key, body) in &inside {
        s.put_bytes(key, body.clone(), "application/octet-stream")
            .await
            .expect("put_bytes");
    }
    s.put_bytes(&outside, vec![9u8], "application/octet-stream")
        .await
        .expect("put_bytes outside");

    let mut entries = Vec::new();
    let mut listing = std::pin::pin!(s.list(&prefix).expect("list"));
    while let Some(entry) = listing.next().await {
        entries.push(entry.expect("list entry"));
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let listed: Vec<(String, i64)> = entries
        .iter()
        .map(|e| (e.key.clone(), e.byte_size))
        .collect();
    let expected: Vec<(String, i64)> = inside
        .iter()
        .map(|(key, body)| (key.clone(), body.len() as i64))
        .collect();
    assert_eq!(listed, expected, "listing does not match what was written");
    for entry in &entries {
        assert!(
            entry.last_modified > UNIX_EPOCH,
            "{} carries no timestamp",
            entry.key
        );
    }
    eprintln!("list           {prefix} -> {listed:?}");

    for (key, _) in &inside {
        s.delete(key).await.expect("delete");
    }
    s.delete(&outside).await.expect("delete outside");

    let mut swept = std::pin::pin!(s.list(&prefix).expect("list after delete"));
    assert!(swept.next().await.is_none(), "prefix is empty after delete");
}

#[tokio::test]
async fn a_cancelled_upload_discards_its_parts_instead_of_leaving_them_billed() {
    let s = storage();
    ensure_bucket(&s, &reqwest::Client::new()).await;

    let prefix = unique("cancelled");
    let key = format!("{prefix}/interrupted.mp3");

    // Past one part, then stall forever: a client that stopped sending.
    let chunk = vec![7u8; 512 * 1024];
    let full_parts = MULTIPART_PART_SIZE / chunk.len() + 2;
    let source = futures_util::stream::iter(
        (0..full_parts).map(move |_| Ok(bytes::Bytes::from(chunk.clone()))),
    )
    .chain(futures_util::stream::once(async {
        std::future::pending::<()>().await;
        unreachable!("the stall is the point")
    }));

    let events = Events::default();
    {
        let _capture = tracing_subscriber::registry()
            .with(events.clone())
            .set_default();
        // Dropped where it stands, as a request timeout drops it.
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(1500),
            s.put_stream(&key, "audio/mpeg", source),
        )
        .await;
        assert!(cancelled.is_err(), "the upload is cancelled, not completed");

        // The abort is handed to a detached task, so give it a turn to run
        // before the capture guard is dropped.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    assert_eq!(
        events.keys_for(CANCELLED),
        vec![key.clone()],
        "a cancelled upload says so, naming the key an operator would need",
    );
    assert_eq!(
        events.keys_for(DISCARDED),
        vec![key.clone()],
        "and its parts are discarded rather than left for a lifecycle rule",
    );
    assert!(
        events.keys_for(DANGLING).is_empty(),
        "the abort itself failed: {:?}",
        events.keys_for(DANGLING),
    );

    assert!(
        s.head(&key).await.expect("head").is_none(),
        "a cancelled upload materializes no object",
    );
    eprintln!("put_stream(cancelled) {key} -> parts discarded");
}

#[tokio::test]
async fn an_upload_moving_slower_than_the_read_bound_completes() {
    let direct = storage();
    ensure_bucket(&direct, &reqwest::Client::new()).await;
    let key = unique("paced.bin");
    let body: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    // 16 KiB every 10 ms: the megabyte takes well over three read bounds.
    let (proxy, _) = proxy(|_| Carry {
        request_pace: Some(16 * 1024),
        ..Carry::default()
    })
    .await;
    let paced = proxied(
        proxy,
        StorageConfig {
            read_timeout: Duration::from_millis(200),
            ..StorageConfig::default()
        },
    );

    paced
        .put_bytes(&key, body.clone(), "application/octet-stream")
        .await
        .expect("an upload that moves is not cut by the read bound");
    let stored = direct.get_bytes(&key).await.expect("get_bytes");
    assert_eq!(stored.as_ref(), body.as_slice(), "the object is the body");

    direct.delete(&key).await.expect("delete");
}
