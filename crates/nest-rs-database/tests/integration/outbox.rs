//! Covers `src/outbox.rs` — the crate names the binding for the store it declares.

use std::sync::Arc;

use nest_rs_core::{App, injectable, module};
use nest_rs_database::OutboxStore;

#[expect(
    dead_code,
    reason = "the store is declared for the container to resolve, never read"
)]
#[injectable]
struct ReadsTheOutbox {
    #[inject]
    store: Arc<dyn OutboxStore>,
}

#[module(providers = [ReadsTheOutbox])]
struct OutboxWithNoBinding;

#[tokio::test]
async fn a_provider_injecting_the_outbox_store_with_no_binding_is_told_the_binding() {
    let Err(refused) = App::builder().module::<OutboxWithNoBinding>().build().await else {
        panic!("no module binds the store");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains("`nest_rs::seaorm::SeaOrmDatabaseModule`"),
        "{said}"
    );
}
