//! Presign, list and streamed-upload round-trips through [`Storage`]
//! (`src/client.rs`) against the live S3-compatible server.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use nest_rs_storage::{MULTIPART_PART_SIZE, Storage, StorageConfig, StorageError, TARGET};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

fn storage() -> Storage {
    let mut config = StorageConfig::default();
    // Honor the documented `NESTRS_STORAGE__ENDPOINT` override; the default
    // (dev-container RustFS) stands when it is unset.
    if let Ok(endpoint) = std::env::var(nest_rs_config::var_name("storage", "ENDPOINT")) {
        config.endpoint = endpoint;
    }
    Storage::new(Arc::new(config))
}

/// Best-effort bucket creation: a presigned PUT on the bucket root is an S3
/// `CreateBucket`. A 2xx means created, a 409 means it already exists — both are
/// fine. Anything else we surface for visibility but don't fail on (the object
/// round-trip below is the real assertion).
async fn ensure_bucket(s: &Storage, http: &reqwest::Client) {
    let url = s
        .presign_put("", Duration::from_secs(60))
        .await
        .expect("presign bucket-root PUT");
    match http.put(&url).send().await {
        Ok(resp) => eprintln!("ensure_bucket: {} ({})", resp.status(), s.bucket_name()),
        Err(e) => eprintln!("ensure_bucket: request error (ignored): {e}"),
    }
}

/// A key no other run can collide with — the bucket is shared with every other
/// suite in the devcontainer, and `list` asserts on an exact set.
fn unique(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("e2e-{}-{}-{}", std::process::id(), nanos, label)
}

/// The two things the client can say about an interrupted upload's parts.
/// Copied rather than shared because they are log *messages*: exporting them
/// would make a wording change a breaking API change.
const DISCARDED: &str = "discarded the parts of an interrupted multipart upload";
const DANGLING: &str = "multipart upload left dangling parts";
const CANCELLED: &str = "multipart upload was cancelled mid-flight; discarding its parts";

/// The `nest_rs::storage` events a call emitted, as `(message, key)`.
///
/// Needed because the abort has **no S3-observable effect**: an interrupted
/// multipart upload materializes no object whether or not its parts were
/// discarded, and the one API that would tell them apart
/// (`ListMultipartUploads`) is not on `object_store`'s surface. Without this
/// witness, deleting an `abort_upload` call would leave every other assertion
/// in the abort test passing.
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

    // 1. Upload via a presigned PUT URL with a raw HTTP client.
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

    // 2. Read back via a presigned GET URL (raw HTTP).
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

    // 3. Read back server-side through object_store (get_bytes).
    let server_bytes = s.get_bytes(key).await.expect("get_bytes");
    assert_eq!(server_bytes.as_ref(), body.as_slice(), "get_bytes mismatch");
    eprintln!(
        "get_bytes      {} -> {} bytes match",
        key,
        server_bytes.len()
    );

    // 4. head: size is reported (content-type is the documented object_store gap).
    let info = s.head(key).await.expect("head").expect("object present");
    assert_eq!(info.byte_size, body.len() as i64, "head size mismatch");
    eprintln!("head           {} -> size={}", key, info.byte_size);

    // 5. head on a missing object returns Ok(None).
    let absent = s
        .head(&unique("does-not-exist"))
        .await
        .expect("head absent");
    assert!(absent.is_none(), "expected None for absent object");
    eprintln!("head(absent)   -> None (Ok)");

    // 6. put_bytes server-side, then read it back, proving the write path too.
    let key2 = unique("variant.webp");
    let key2 = key2.as_str();
    s.put_bytes(key2, vec![1, 2, 3, 4], "image/webp")
        .await
        .expect("put_bytes");
    let rt = s.get_bytes(key2).await.expect("get_bytes key2");
    assert_eq!(rt.as_ref(), &[1, 2, 3, 4], "put_bytes round-trip mismatch");
    eprintln!("put_bytes/get  {} -> 4 bytes match", key2);

    // The bucket is shared with every other suite in the devcontainer, and
    // `list` asserts on an exact set — so this test cleans up after itself the
    // way its four siblings already do.
    s.delete(key).await.expect("delete key");
    s.delete(key2).await.expect("delete key2");
}

#[tokio::test]
async fn put_stream_uploads_in_parts_and_keeps_the_content_type() {
    let s = storage();
    let http = reqwest::Client::new();
    ensure_bucket(&s, &http).await;

    let key = unique("stream.mp3");
    // Past the 5 MiB minimum part size, so the upload is a real multipart one
    // (two parts) rather than a single-part upload that would prove nothing.
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

    // `head` cannot report the content type (object_store's ObjectMeta drops
    // it), so read it off the wire — which is where it matters anyway.
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

    // A source that yields nothing still creates the object: the upload ships an
    // empty tail part, because a multipart upload with no part at all is
    // rejected on completion.
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

/// What a failed streamed upload must not leave behind. S3 bills the parts of
/// an interrupted multipart upload until something removes them, and they are
/// invisible to a `list` — so "nothing happened" is exactly what a missing
/// abort looks like from the outside, until the invoice arrives.
///
/// Three separate claims, and the payload is sized so the third is not vacuous:
/// the source fails only after a part has really been shipped to the store.
#[tokio::test]
async fn a_failing_source_surfaces_its_own_error_and_leaves_no_object_or_parts_behind() {
    let s = storage();
    ensure_bucket(&s, &reqwest::Client::new()).await;

    let prefix = unique("aborted");
    let key = format!("{prefix}/interrupted.mp3");

    const SOURCE_FAILURE: &str = "the reader went away mid-upload";
    let chunk = vec![7u8; 512 * 1024];
    // Past one part, so `put_part` has landed before the failure — an upload
    // that never reached the store has no parts to discard and would prove
    // nothing about aborting.
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

    // 1. The source's own failure comes back, tagged as the source's — not
    //    swallowed, and not reported as the store's.
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

    // 2. Nothing readable is left at the key — not through `head`, and not in
    //    the listing either.
    assert!(
        s.head(&key).await.expect("head").is_none(),
        "an interrupted upload must not materialize an object",
    );
    let mut listing = std::pin::pin!(s.list(&prefix).expect("list"));
    assert!(
        listing.next().await.is_none(),
        "and nothing is listed under {prefix}",
    );

    // 3. The parts that *were* shipped are discarded, once, for this key.
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

/// The other half of the claim above: a *successful* upload never aborts. A
/// discard on the success path would be a silent data-loss bug, and the witness
/// that catches a deleted abort would not catch a spurious one.
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
    // Shares the prefix's characters but not its path segments — `list` must
    // not return it.
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

/// The interruption every returning path already covered, and the one that does
/// not return: cancellation.
///
/// A request timeout (30 s by default) or a client hanging up drops the
/// `put_stream` future mid-part. No `.await` inside it runs again, so
/// `abort_upload` was unreachable — the parts stayed on the store, billed, and
/// **nothing at all was logged**. It is the likeliest interruption for exactly
/// the uploads streaming exists for: a slow link against a default timeout.
///
/// The suite's other witness cannot see this. `a_failing_source_…` covers the
/// source-error path, which returns; deleting the cancellation handling leaves
/// every one of its assertions passing.
#[tokio::test]
async fn a_cancelled_upload_discards_its_parts_instead_of_leaving_them_billed() {
    let s = storage();
    ensure_bucket(&s, &reqwest::Client::new()).await;

    let prefix = unique("cancelled");
    let key = format!("{prefix}/interrupted.mp3");

    // Past one part, so the store really holds something to discard, then stall
    // forever — the shape of a client that stopped sending.
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
        // Cancellation, exactly as the transport's request timeout produces it:
        // the future is dropped where it stands.
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
