//! `StorageModule` (`src/module.rs`) booted over `memory:///`: the store opens
//! with no network, and the client reads its URL back.

use crate::booted;

#[tokio::test]
async fn memory_boots_with_no_network_and_shows_its_url() {
    let (_app, storage) = booted().await;
    assert_eq!(storage.url(), "memory:///uploads");
}
