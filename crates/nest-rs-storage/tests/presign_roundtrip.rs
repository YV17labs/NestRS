//! Live presign round-trip against an S3-compatible server (RustFS in the dev
//! container). Proves that `object_store`'s `Signer` produces URLs a plain HTTP
//! client can PUT to and GET from, in path-style over plain HTTP.
//!
//! Ignored by default — it needs a reachable server. Run it explicitly:
//!
//! ```bash
//! cargo test -p nest-rs-storage --test presign_roundtrip -- --ignored --nocapture
//! ```
//!
//! Config comes from `StorageConfig::default()`, which targets the dev
//! container's RustFS (`http://rustfs:9000`, `nestrs`/`nestrs`, bucket
//! `nestrs`, path-style). Override via `NESTRS_STORAGE__*` is not used here so
//! the test is self-contained.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_storage::{Storage, StorageConfig};

fn storage() -> Storage {
    Storage::new(Arc::new(StorageConfig::default()))
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

#[tokio::test]
#[ignore = "needs a live RustFS/S3 server (dev container)"]
async fn presign_put_get_round_trip() {
    let s = storage();
    let http = reqwest::Client::new();
    ensure_bucket(&s, &http).await;

    let key = "spike/object_store/hello.txt";
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
    assert!(got.status().is_success(), "presigned GET failed: {}", got.status());
    let got_bytes = got.bytes().await.expect("GET body").to_vec();
    assert_eq!(got_bytes, body, "presigned GET bytes mismatch");
    eprintln!("GET(presigned) {} -> {} bytes match", key, got_bytes.len());

    // 3. Read back server-side through object_store (get_bytes).
    let server_bytes = s.get_bytes(key).await.expect("get_bytes");
    assert_eq!(server_bytes.as_ref(), body.as_slice(), "get_bytes mismatch");
    eprintln!("get_bytes      {} -> {} bytes match", key, server_bytes.len());

    // 4. head: size is reported (content-type is the documented object_store gap).
    let info = s.head(key).await.expect("head").expect("object present");
    assert_eq!(info.byte_size, body.len() as i64, "head size mismatch");
    eprintln!("head           {} -> size={}", key, info.byte_size);

    // 5. head on a missing object returns Ok(None).
    let absent = s.head("spike/object_store/does-not-exist").await.expect("head absent");
    assert!(absent.is_none(), "expected None for absent object");
    eprintln!("head(absent)   -> None (Ok)");

    // 6. put_bytes server-side, then read it back, proving the write path too.
    let key2 = "spike/object_store/variant.webp";
    s.put_bytes(key2, vec![1, 2, 3, 4], "image/webp")
        .await
        .expect("put_bytes");
    let rt = s.get_bytes(key2).await.expect("get_bytes key2");
    assert_eq!(rt.as_ref(), &[1, 2, 3, 4], "put_bytes round-trip mismatch");
    eprintln!("put_bytes/get  {} -> 4 bytes match", key2);
}
