//! The flow-owning [`SocialProvider`] trait (`src/provider.rs`): its default
//! `authorize`/`exchange` delegate to the configured `client()`.

use nest_rs_authn::{JwtOptions, JwtService, OAuth2Client, OAuth2Config, TokenSet};
use nest_rs_social::{ProfileFuture, SocialProfile, SocialProvider};

/// A provider that keeps the default `authorize`/`exchange` and implements only
/// `profile` — the shape a real first-party provider takes, so exercising its
/// `authorize` proves the default delegates to `client()`.
struct StubProvider {
    client: OAuth2Client,
}

impl SocialProvider for StubProvider {
    fn key(&self) -> &'static str {
        "stub"
    }
    fn client(&self) -> &OAuth2Client {
        &self.client
    }
    fn profile<'a>(&'a self, _tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move { Ok(SocialProfile::new("stub", "1")) })
    }
}

fn oauth_config() -> OAuth2Config {
    OAuth2Config {
        client_id: "id".into(),
        client_secret: "secret".into(),
        auth_url: "https://provider.example/authorize".into(),
        token_url: "https://provider.example/token".into(),
        redirect_url: "https://app.example/callback".into(),
        userinfo_url: "https://provider.example/userinfo".into(),
        scopes: vec!["read".into()],
    }
}

#[test]
fn default_authorize_delegates_to_the_configured_client() {
    let provider = StubProvider {
        client: OAuth2Client::new(oauth_config()).expect("valid client"),
    };
    let jwt =
        JwtService::new(JwtOptions::new("social-int-tests-padded-to-32-bytes")).expect("hmac jwt");

    let authorization = provider
        .authorize(&jwt)
        .expect("the trait default drives the shared flow through client()");

    assert!(
        authorization
            .url
            .starts_with("https://provider.example/authorize"),
        "redirect must hit the client's configured provider, got {}",
        authorization.url,
    );
    assert!(!authorization.transaction.is_empty());
}
