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
async fn a_secret_the_store_refuses_opens_with_one_warn_naming_the_credentials() {
    let logs = nest_rs_testing::LogCapture::install();
    let storage = Storage::connect(&StorageConfig {
        secret_key: "not-the-secret".into(),
        ..config()
    })
    .await
    .expect("a store refusing to list is no reason to refuse the boot");
    let warned = logs.expect_one(
        nest_rs_storage::TARGET,
        "the object store refused to list its bucket — its credentials are refused, or its \
         policy grants no s3:ListBucket on the prefix; read as answering, and the next call will \
         say which",
    );
    assert_eq!(warned.level, "warn");
    assert!(
        warned
            .field("credentials")
            .is_some_and(|set| set.contains(&nest_rs_config::var_name("storage", "SECRET_KEY"))),
        "{:?}",
        warned.fields
    );
    let refused = storage
        .head("anything")
        .await
        .expect_err("and the first call says the store refused it");
    assert!(matches!(refused, StorageError::Head { .. }), "{refused:?}");
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
