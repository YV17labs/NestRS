//! Every call of [`Storage`](nest_rs_storage::Storage) (`src/client.rs`) on
//! the in-process store, as an app's service makes it.

use std::time::{Duration, SystemTime};

use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use nest_rs_storage::{SignedPut, StorageError};

use crate::booted;

#[tokio::test]
async fn bytes_round_trip_and_an_absent_key_reads_as_none() {
    let (_app, storage) = booted().await;
    storage
        .put_bytes("covers/a.png", vec![1_u8, 2, 3], "image/png")
        .await
        .expect("put");
    assert_eq!(
        storage
            .get_bytes("covers/a.png")
            .await
            .expect("get")
            .as_ref(),
        [1, 2, 3]
    );
    assert_eq!(
        storage
            .head("covers/a.png")
            .await
            .expect("head")
            .map(|meta| meta.byte_size),
        Some(3)
    );
    assert!(storage.head("covers/b.png").await.expect("head").is_none());
}

#[tokio::test]
async fn head_reads_back_the_type_an_upload_stored() {
    let (_app, storage) = booted().await;
    let before = SystemTime::now();
    storage
        .put_bytes("covers/a.png", vec![1_u8, 2, 3], "image/png")
        .await
        .expect("put");
    storage
        .put_stream(
            "audio/a.mp3",
            "audio/mpeg",
            futures_util::stream::iter([Ok(Bytes::from_static(b"mp3"))]),
        )
        .await
        .expect("put_stream");

    let png = storage
        .head("covers/a.png")
        .await
        .expect("head")
        .expect("the object is stored");
    assert_eq!(png.content_type.as_deref(), Some("image/png"));
    assert_eq!(png.byte_size, 3);
    assert!(png.etag.is_some(), "{png:?}");
    assert!(
        png.last_modified >= before - Duration::from_secs(1),
        "{png:?}"
    );

    let mp3 = storage
        .head("audio/a.mp3")
        .await
        .expect("head")
        .expect("the object is stored");
    assert_eq!(mp3.content_type.as_deref(), Some("audio/mpeg"));
}

#[tokio::test]
async fn a_streamed_upload_reads_back_as_a_stream() {
    let (_app, storage) = booted().await;
    let chunks: Vec<std::io::Result<Bytes>> = vec![
        Ok(Bytes::from_static(b"stream")),
        Ok(Bytes::from_static(b"ed")),
    ];
    storage
        .put_stream(
            "audio/a.mp3",
            "audio/mpeg",
            futures_util::stream::iter(chunks),
        )
        .await
        .expect("put_stream");
    let read: Vec<u8> = storage
        .get_stream("audio/a.mp3")
        .await
        .expect("get_stream")
        .map_ok(|chunk| chunk.to_vec())
        .try_concat()
        .await
        .expect("the stream reads");
    assert_eq!(read, b"streamed");
}

#[tokio::test]
async fn list_matches_path_segments_and_delete_is_idempotent() {
    let (_app, storage) = booted().await;
    for key in [
        "posts/cover/a.png",
        "posts/cover/nested/b.png",
        "posts/cover-2.png",
    ] {
        storage.put_bytes(key, "x", "image/png").await.expect("put");
    }
    let mut keys: Vec<String> = storage
        .list("posts/cover")
        .map_ok(|entry| entry.key)
        .try_collect()
        .await
        .expect("list");
    keys.sort();
    assert_eq!(keys, ["posts/cover/a.png", "posts/cover/nested/b.png"]);

    storage.delete("posts/cover/a.png").await.expect("delete");
    storage
        .delete("posts/cover/a.png")
        .await
        .expect("an absent key deletes");
    let left = storage.list("posts/cover").count().await;
    assert_eq!(left, 1);
}

#[tokio::test]
async fn presigning_answers_unsupported_naming_the_backend() {
    let (_app, storage) = booted().await;
    for signed in [
        storage
            .presign_put(
                "a.png",
                &SignedPut::new("image/png"),
                Duration::from_secs(60),
            )
            .await,
        storage.presign_get("a.png", Duration::from_secs(60)).await,
    ] {
        let refused = signed.expect_err("memory signs nothing");
        assert!(
            matches!(
                refused,
                StorageError::Unsupported {
                    operation: "presign",
                    backend: "memory"
                }
            ),
            "{refused:?}"
        );
    }
}
