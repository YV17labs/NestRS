//! A download's body against the live store, through a proxy that stalls it
//! (`src/transfer.rs`): bounded by its stall while its reader waits, resumed
//! from where it stopped, and never cut by its size or its reader's pauses.

use std::pin::pin;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use nest_rs_storage::{Storage, StorageConfig};

use crate::{Carry, Then, Until, ensure_bucket, proxied, proxy, storage, unique};

/// A megabyte no two offsets of which share a byte pattern for long, stored
/// under a key of its own.
async fn stored(label: &str) -> (Storage, String, Vec<u8>) {
    let direct = storage();
    ensure_bucket(&direct, &reqwest::Client::new()).await;
    let key = unique(label);
    let body: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    direct
        .put_bytes(&key, body.clone(), "application/octet-stream")
        .await
        .expect("put_bytes");
    (direct, key, body)
}

fn reading_within(read_timeout: Duration) -> StorageConfig {
    StorageConfig {
        read_timeout,
        ..StorageConfig::default()
    }
}

#[tokio::test]
async fn a_download_stalled_mid_body_is_cut_at_the_read_bound_and_resumed() {
    let (direct, key, body) = stored("stalled.bin").await;
    let (proxy, connections) = proxy(|n| Carry {
        answer_ends: (n == 0).then_some((Until::Bytes(64 * 1024), Then::Hold)),
        ..Carry::default()
    })
    .await;
    let bound = Duration::from_millis(300);
    let proxied = proxied(proxy, reading_within(bound));

    let started = Instant::now();
    let got = tokio::time::timeout(Duration::from_secs(10), proxied.get_bytes(&key))
        .await
        .expect("a stall is cut at the read bound, not waited out")
        .expect("the download resumes and completes");
    assert_eq!(
        got.as_ref(),
        body.as_slice(),
        "the resumed body is the object"
    );
    assert!(started.elapsed() >= bound, "{:?}", started.elapsed());
    assert!(
        connections.load(Ordering::SeqCst) >= 2,
        "the stalled transfer was resumed on a connection of its own"
    );

    direct.delete(&key).await.expect("delete");
}

#[tokio::test]
async fn a_reader_pausing_past_the_read_bound_is_never_cut() {
    let (direct, key, body) = stored("paused.bin").await;
    let (proxy, connections) = proxy(|_| Carry::default()).await;
    let proxied = proxied(proxy, reading_within(Duration::from_millis(200)));

    let mut stream = pin!(proxied.get_stream(&key).await.expect("get_stream"));
    let mut got = Vec::new();
    let first = stream
        .next()
        .await
        .expect("a first chunk")
        .expect("the first chunk reads");
    got.extend_from_slice(&first);
    tokio::time::sleep(Duration::from_millis(600)).await;
    while let Some(chunk) = stream.next().await {
        got.extend_from_slice(&chunk.expect("the transfer goes on after the pause"));
    }
    assert_eq!(got, body, "the whole object, once");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "the pause was not taken for a stall and resumed"
    );

    direct.delete(&key).await.expect("delete");
}

#[tokio::test]
async fn a_resumed_download_stalled_before_its_first_byte_fails_naming_the_read_bound() {
    let (direct, key, _) = stored("stalled-again.bin").await;
    let (proxy, connections) = proxy(|n| Carry {
        answer_ends: Some(if n == 0 {
            (Until::Bytes(64 * 1024), Then::Hold)
        } else {
            (Until::BodyStarts, Then::Hold)
        }),
        ..Carry::default()
    })
    .await;
    let proxied = proxied(proxy, reading_within(Duration::from_millis(300)));

    let refused = tokio::time::timeout(Duration::from_secs(10), proxied.get_bytes(&key))
        .await
        .expect("two stalls are two read bounds, not a retry budget")
        .expect_err("a store that sends nothing after a resume fails the download");
    let chain = nest_rs_core::error_message(&refused);
    assert!(
        chain.contains(&nest_rs_config::var_name("storage", "READ_TIMEOUT_SECS")),
        "{chain}"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 2, "one resume, no more");

    direct.delete(&key).await.expect("delete");
}

#[tokio::test]
async fn an_empty_object_reads_as_no_bytes() {
    let direct = storage();
    ensure_bucket(&direct, &reqwest::Client::new()).await;
    let key = unique("empty.bin");
    direct
        .put_bytes(&key, Vec::new(), "application/octet-stream")
        .await
        .expect("put_bytes");

    assert!(direct.get_bytes(&key).await.expect("get_bytes").is_empty());
    let mut stream = pin!(direct.get_stream(&key).await.expect("get_stream"));
    while let Some(chunk) = stream.next().await {
        assert!(chunk.expect("an empty body reads").is_empty());
    }
    assert!(stream.next().await.is_none(), "an ended stream stays ended");

    direct.delete(&key).await.expect("delete");
}

/// `object_store` resumes a broken body itself only within half the operation
/// budget; past it, the client does.
#[tokio::test]
async fn a_download_broken_past_the_retries_is_resumed_and_says_so() {
    let (direct, key, body) = stored("broken.bin").await;
    let budget = Duration::from_secs(1);
    // 16 KiB every 10 ms: the 960 KiB before the break take over half the budget.
    let (proxy, connections) = proxy(|n| Carry {
        answer_pace: (n == 0).then_some(16 * 1024),
        answer_ends: (n == 0).then_some((Until::Bytes(960 * 1024), Then::Close)),
        ..Carry::default()
    })
    .await;
    let proxied = proxied(
        proxy,
        StorageConfig {
            operation_timeout: budget,
            ..StorageConfig::default()
        },
    );
    let logs = nest_rs_testing::LogCapture::install();

    let got = proxied
        .get_bytes(&key)
        .await
        .expect("the download resumes past the break");
    assert_eq!(
        got.as_ref(),
        body.as_slice(),
        "the resumed body is the object"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 2, "one resume");
    let resumed = logs.expect_one(
        nest_rs_storage::TARGET,
        "download resumed where its body stopped",
    );
    assert_eq!(resumed.level, "warn");
    assert_eq!(resumed.field("key").as_deref(), Some(key.as_str()));
    assert!(
        resumed
            .field("error")
            .is_some_and(|error| !error.is_empty())
    );

    direct.delete(&key).await.expect("delete");
}
