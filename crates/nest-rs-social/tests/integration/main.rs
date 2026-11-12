//! Integration coverage for the social provider seam: link-time entry
//! submission, the module that opens each provider, the flow-owning
//! trait's default delegation, the authorization request's answer and the
//! redirect URI's strategy.
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
mod redirect;
mod registry;
mod strategy;

use std::sync::Arc;

use nest_rs_core::AppBuilder;
use nest_rs_oauth_client::{AuthorizationResponse, OAuthClient, OAuthClientConfig, TokenSet};
use nest_rs_social::{
    GithubSocialConfig, GoogleSocialConfig, ProfileFuture, SocialProfile, SocialProvider,
    SocialProviderEntry, SocialRegistry, TokenFuture,
};

const KEY: &str = "callback-stub";
const SECRET: &str = "social-strategy-secret-padded-32b";
/// The issuer and the audience of the app signing users in, which its
/// `JwtService` requires.
const APP: &str = "https://app.example";
const CODE: &str = "the-code-never-logged";
const MCP: &str = "https://mcp.example.com/mcp";

/// A provider whose exchange and profile answer in process: what the strategy
/// hands it, it checks.
struct StubProvider {
    client: OAuthClient,
}

impl SocialProvider for StubProvider {
    fn key(&self) -> &'static str {
        KEY
    }

    fn client(&self) -> &OAuthClient {
        &self.client
    }

    fn exchange(&self, response: AuthorizationResponse) -> TokenFuture<'_> {
        Box::pin(async move {
            assert_eq!(response.code().expose_secret(), CODE);
            Ok(TokenSet::new("provider-access-token").with_refresh_token("provider-refresh-token"))
        })
    }

    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move {
            assert_eq!(tokens.access_token.expose_secret(), "provider-access-token");
            Ok(SocialProfile::new(KEY, "provider-subject-42"))
        })
    }
}

fn stub_provider() -> StubProvider {
    StubProvider {
        client: OAuthClient::new(OAuthClientConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://provider.example/authorize".into(),
            token_url: "https://provider.example/token".into(),
            redirect_url: "https://app.example/social/callback-stub/callback".into(),
            userinfo_url: "https://provider.example/userinfo".into(),
            scopes: vec!["read".into()],
            authorization_parameters: Default::default(),
            tls: nest_rs_config::ClientTls::default(),
        })
        .expect("a valid client"),
    }
}

/// The module opening [`StubProvider`].
#[nest_rs_core::module]
struct StubSocialModule;

nest_rs_core::inventory::submit! {
    SocialProviderEntry::new::<StubProvider, StubSocialModule>(
        KEY,
        "SOCIAL__CALLBACK_STUB",
        |_container| Ok(Some(Arc::new(stub_provider()))),
    )
}

fn linked_keys() -> Vec<&'static str> {
    nest_rs_core::inventory::iter::<SocialProviderEntry>()
        .map(|entry| entry.key())
        .collect()
}

fn seeded_github_config() -> GithubSocialConfig {
    GithubSocialConfig {
        client_id: "seeded-client".into(),
        client_secret: "seeded-secret".into(),
        redirect_url: "https://acme.example.com/auth/github/callback".into(),
        scopes: Vec::new(),
        authorization_parameters: Default::default(),
    }
}

fn seeded_google_config() -> GoogleSocialConfig {
    GoogleSocialConfig {
        client_id: "seeded-client".into(),
        client_secret: "seeded-secret".into(),
        redirect_url: "https://acme.example.com/auth/google/callback".into(),
        scopes: Vec::new(),
        authorization_parameters: Default::default(),
    }
}

async fn registry_keys(app: AppBuilder) -> Vec<&'static str> {
    let app = app.build().await.expect("boots");
    app.container()
        .get::<SocialRegistry>()
        .expect("SocialModule provides the registry")
        .keys()
}
