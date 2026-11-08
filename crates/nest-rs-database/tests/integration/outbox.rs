//! Covers `src/outbox.rs` — the crate names the binding for the store it
//! declares, and a store the app lists itself is named in its place.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{AccessGraphError, App, injectable, module};
use nest_rs_database::{OutboxClaim, OutboxError, OutboxStore};

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

/// `AppOutboxModule` below lists the store in this binary, a module this app
/// does not reach, so the access graph says it: the crate's line all the same.
/// With no module listing it, `tests/unlisted` holds the unmet dependency's.
#[tokio::test]
async fn a_provider_injecting_the_outbox_store_with_no_binding_is_told_the_binding() {
    let Err(refused) = App::builder().module::<OutboxWithNoBinding>().build().await else {
        panic!("no module binds the store");
    };
    let violation = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not an access violation: {refused:#}"));
    assert!(
        violation
            .remedy
            .is_some_and(|remedy| remedy.contains("`nest_rs::seaorm::SeaOrmDatabaseModule`")),
        "{violation}"
    );
}

/// An outbox store the app binds itself, in a module of its own.
#[injectable]
#[derive(Default)]
struct AppOutbox;

impl OutboxStore for AppOutbox {
    fn arm(&self) -> futures_util::future::BoxFuture<'_, Result<(), OutboxError>> {
        Box::pin(async { Ok(()) })
    }

    fn written(&self, sweep: Duration) -> futures_util::future::BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(sweep))
    }

    fn claim(
        &self,
        _: usize,
    ) -> futures_util::future::BoxFuture<'_, Result<Box<dyn OutboxClaim>, OutboxError>> {
        Box::pin(async {
            Err(OutboxError::Missing {
                table: "app_outbox",
                remedy: "the app's own migration",
            })
        })
    }
}

#[module(providers = [AppOutbox as dyn OutboxStore])]
struct AppOutboxModule;

#[module(imports = [AppOutboxModule, OutboxWithNoBinding])]
struct OutboxListedBesideItsReader;

/// The module listing the store is the fix, whatever the port names: a driver's
/// binding imported beside it would register the key twice.
#[tokio::test]
async fn a_reader_beside_the_module_listing_the_outbox_store_is_told_that_module() {
    let Err(refused) = App::builder()
        .module::<OutboxListedBesideItsReader>()
        .build()
        .await
    else {
        panic!("the reader imports nothing");
    };
    let violation = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not an access violation: {refused:#}"));
    assert_eq!(
        (violation.consumer, violation.owner),
        ("ReadsTheOutbox", "AppOutboxModule")
    );
    let said = violation.to_string();
    assert!(said.contains("— add `AppOutboxModule` to"), "{said}");
    assert!(!said.contains("SeaOrmDatabaseModule"), "{said}");
}

#[module(imports = [AppOutboxModule], providers = [ReadsTheOutbox])]
struct ReaderOverItsOutbox;

#[tokio::test]
async fn following_that_sentence_boots_on_the_apps_outbox_store() {
    let app = App::builder()
        .module::<ReaderOverItsOutbox>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the import is the fix: {refused:#}"));
    assert!(app.container().get::<ReadsTheOutbox>().is_some());
    assert!(app.container().get::<AppOutbox>().is_some());
}
