//! Covers `src/module.rs` — the `for_root` seam, executed.

use std::sync::Arc;

use nest_rs_core::{App, module};
use nest_rs_oauth_client::{OAuthClient, OAuthClientConfig, OAuthClientModule, OAuthClientSetup};

use super::config::valid_config;

/// A base distinct from every other fixture's, so an assertion can only pass through it.
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

    // The URL the client builds proves the pinned base reached its constructor.
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
