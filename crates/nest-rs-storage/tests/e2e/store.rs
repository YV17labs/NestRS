//! What the live store answers the boot's proof (`src/store.rs`), and a key
//! confined under the prefix of the store's URL.

use futures_util::TryStreamExt;
use nest_rs_storage::{Storage, StorageConfig, StorageError};

use crate::{config, storage, unique};

#[tokio::test]
async fn a_bucket_that_does_not_exist_refuses_to_open_naming_the_url_variable() {
    let url = format!("s3://{}", unique("absent"));
    let Err(refused) = Storage::connect(&StorageConfig {
        url: url.clone(),
        ..config()
    })
    .await
    else {
        panic!("a bucket that does not exist is not proved");
    };
    assert!(
        matches!(refused, StorageError::Refused { .. }),
        "{refused:?}"
    );
    let chain = nest_rs_core::error_message(&refused);
    assert!(
        chain.contains(&format!(
            "the bucket {} names does not exist",
            nest_rs_config::var_name("storage", "URL")
        )) && chain.contains(&url),
        "{chain}"
    );
}

#[tokio::test]
async fn a_secret_the_store_rejects_refuses_to_open_naming_the_credentials() {
    let Err(refused) = Storage::connect(&StorageConfig {
        secret_key: "not-the-secret".into(),
        ..config()
    })
    .await
    else {
        panic!("every real call would fail as the proof's listing did");
    };
    assert!(
        matches!(refused, StorageError::Refused { .. }),
        "{refused:?}"
    );
    let chain = nest_rs_core::error_message(&refused);
    assert!(
        chain.contains(&nest_rs_config::var_name("storage", "SECRET_KEY")),
        "{chain}"
    );
}

#[tokio::test]
async fn an_access_key_the_store_does_not_know_refuses_to_open_naming_the_credentials() {
    let Err(refused) = Storage::connect(&StorageConfig {
        access_key: "not-the-key".into(),
        ..config()
    })
    .await
    else {
        panic!("every real call would fail as the proof's listing did");
    };
    assert!(
        matches!(refused, StorageError::Refused { .. }),
        "{refused:?}"
    );
    let chain = nest_rs_core::error_message(&refused);
    assert!(
        chain.contains(&nest_rs_config::var_name("storage", "ACCESS_KEY")),
        "{chain}"
    );
}

#[tokio::test]
async fn a_prefixed_url_confines_every_key_under_its_prefix() {
    let prefix = unique("confined");
    let whole = storage().await;
    let confined = Storage::connect(&StorageConfig {
        url: format!("{}/{prefix}", config().url),
        ..config()
    })
    .await
    .expect("the prefix opens");

    confined
        .put_bytes("a.txt", "inside", "text/plain")
        .await
        .expect("put under the prefix");
    let stored = whole
        .get_bytes(&format!("{prefix}/a.txt"))
        .await
        .expect("the key sits under the prefix in the bucket");
    assert_eq!(stored.as_ref(), b"inside");
    let listed: Vec<String> = confined
        .list("")
        .map_ok(|entry| entry.key)
        .try_collect()
        .await
        .expect("list the prefix");
    assert_eq!(listed, ["a.txt"], "keys read relative to the prefix");

    confined.delete("a.txt").await.expect("delete");
    assert!(
        whole
            .head(&format!("{prefix}/a.txt"))
            .await
            .expect("head")
            .is_none()
    );
}
