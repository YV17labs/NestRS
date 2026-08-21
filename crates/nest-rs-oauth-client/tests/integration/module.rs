//! Covers `src/module.rs` — the `for_root` seam, executed.
//!
//! `OAuthClientModule::for_root` is the only in-code path a consumer has to pin an
//! `OAuthClientConfig`, and until now nothing in either workspace called it. What a
//! compile could never show is what actually matters here: the seam queues a
//! *resolving* factory rather than the struct verbatim, so the pinned base and
//! the `NESTRS_OAUTH_CLIENT__*` cascade are reconciled during the builder's factory
//! phase — a phase only a boot runs.

use std::sync::Arc;

use nest_rs_core::{App, module};
use nest_rs_oauth_client::{OAuthClient, OAuthClientConfig, OAuthClientModule, OAuthClientSetup};

use super::config::valid_config;

/// A base distinct from every other fixture's, so an assertion below can only
/// pass by way of this call.
fn pinned() -> OAuthClientSetup {
    OAuthClientModule::for_root(OAuthClientConfig {
        client_id: "pinned-through-for-root".into(),
        auth_url: "https://pinned.example/authorize".into(),
        ..valid_config()
    })
}

#[module(imports = [pinned()])]
struct PinnedOAuthClientHost;

#[tokio::test]
async fn for_root_pins_the_config_and_provides_a_client_built_from_it() {
    let app = App::builder()
        .module::<PinnedOAuthClientHost>()
        .build()
        .await
        .expect("the pinned-config module boots");

    let config: Arc<OAuthClientConfig> = app
        .container()
        .get()
        .expect("for_root registers the resolved OAuthClientConfig");
    assert_eq!(config.client_id, "pinned-through-for-root");

    // The client is the factory output, not the config: asserting on the URL it
    // *builds* is what proves the pinned base reached the constructor rather
    // than merely being registered beside it.
    let client: Arc<OAuthClient> = app
        .container()
        .get()
        .expect("for_root queues the OAuthClient factory");
    let jwt = crate::jwt();
    let authorization = client.authorize(&jwt, "acme").expect("authorize");
    assert!(
        authorization
            .url
            .starts_with("https://pinned.example/authorize?"),
        "the client was built from the pinned base, got {}",
        authorization.url,
    );
    assert!(
        authorization
            .url
            .contains("client_id=pinned-through-for-root"),
        "got {}",
        authorization.url,
    );
}
