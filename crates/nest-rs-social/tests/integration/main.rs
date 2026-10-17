//! Integration coverage for the social provider seam: link-time entry
//! submission, the module that opens each provider, and the flow-owning
//! trait's default delegation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod module;
mod provider;
mod providers;
mod registry;

use nest_rs_core::AppBuilder;
use nest_rs_social::{GithubSocialConfig, GoogleSocialConfig, SocialProviderEntry, SocialRegistry};

fn linked_keys() -> Vec<&'static str> {
    nest_rs_core::inventory::iter::<SocialProviderEntry>()
        .map(|entry| entry.key)
        .collect()
}

fn seeded_github_config() -> GithubSocialConfig {
    GithubSocialConfig {
        client_id: "seeded-client".into(),
        client_secret: "seeded-secret".into(),
        redirect_url: "https://acme.example.com/auth/github/callback".into(),
        scopes: Vec::new(),
    }
}

fn seeded_google_config() -> GoogleSocialConfig {
    GoogleSocialConfig {
        client_id: "seeded-client".into(),
        client_secret: "seeded-secret".into(),
        redirect_url: "https://acme.example.com/auth/google/callback".into(),
        scopes: Vec::new(),
    }
}

async fn registry_keys(app: AppBuilder) -> Vec<&'static str> {
    let app = app.build().await.expect("boots");
    app.container()
        .get::<SocialRegistry>()
        .expect("SocialModule provides the registry")
        .keys()
}
