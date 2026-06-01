//! [`OAuth2Client`] — the Authorization Code flow (with PKCE), the building block
//! an app's OAuth [`Strategy`](crate::Strategy) is written against.
//!
//! What lives here is provider-agnostic: redirect the browser to the provider,
//! then trade the returned code for an access token. What stays in the app — like
//! a Passport strategy — is the provider's endpoints (passed as [`OAuth2Config`])
//! and what to do with the token (fetch the userinfo, map it to the app's
//! principal).
//!
//! The CSRF `state` and the PKCE verifier must survive the round-trip to the
//! provider and back. Rather than a server-side session, they ride in a
//! short-lived JWT signed by the app's [`JwtService`](crate::JwtService) — set it
//! as a cookie on the redirect, hand it back on the callback. Tamper-proof, and
//! no new storage.

use nestrs_config::config;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::error::AuthError;
use crate::jwt::JwtService;

/// A provider's OAuth2 endpoints and this app's registration with it — a
/// namespaced `#[config]` read from `NESTRS_AUTHN__*` (OAuth lives in the `authn`
/// crate), the source a bare `OAuth2Module` uses. Demo credentials ship in the
/// committed `.env`; real ones come from the environment. `#[serde(default)]` lets
/// an import site spell out only the fields it overrides and close the literal
/// with `..Default::default()`.
// No `Debug`: `client_secret` is a secret and must not leak through a derived format.
#[config(namespace = "authn")]
#[derive(Clone, Default, Deserialize, Validate)]
#[serde(default)]
pub struct OAuth2Config {
    pub client_id: String,
    pub client_secret: String,
    /// The provider's authorization endpoint (where the browser is sent).
    pub auth_url: String,
    /// The provider's token endpoint (where the code is exchanged).
    pub token_url: String,
    /// This app's callback URL, registered with the provider.
    pub redirect_url: String,
    /// The provider's userinfo (profile) endpoint, read with the access token to
    /// identify the caller. OpenID Connect / provider-specific, not core OAuth2.
    pub userinfo_url: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
}

/// The redirect leg of the flow, produced by [`OAuth2Client::authorize`].
pub struct Authorization {
    /// The provider URL to redirect the browser to (a `302 Location`).
    pub url: String,
    /// A signed, short-lived token binding the CSRF state to the PKCE verifier.
    /// Set it as a cookie on the redirect and pass it to
    /// [`exchange`](OAuth2Client::exchange) on the callback.
    pub transaction: String,
}

/// What [`OAuth2Client::authorize`] stashes and [`OAuth2Client::exchange`] reads
/// back, carried as a [`JwtService`]-signed cookie so the client cannot forge it.
#[derive(Serialize, Deserialize)]
struct Transaction {
    csrf: String,
    pkce: String,
    exp: u64,
}

/// A configured OAuth2 client. Construct one per provider from an [`OAuth2Config`].
pub struct OAuth2Client {
    config: OAuth2Config,
    http: oauth2::reqwest::Client,
}

impl OAuth2Client {
    /// Build the client. The HTTP backend refuses redirects — following them
    /// during a token exchange is an SSRF risk (per the `oauth2` crate's own
    /// guidance).
    pub fn new(config: OAuth2Config) -> Result<Self, AuthError> {
        let http = oauth2::reqwest::ClientBuilder::new()
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::Failed(e.to_string()))?;
        Ok(Self { config, http })
    }

    fn basic_client(
        &self,
    ) -> Result<
        BasicClient<
            oauth2::EndpointSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointSet,
        >,
        AuthError,
    > {
        let parse = |s: &str| AuthError::Failed(format!("invalid OAuth URL: {s}"));
        Ok(
            BasicClient::new(ClientId::new(self.config.client_id.clone()))
                .set_client_secret(ClientSecret::new(self.config.client_secret.clone()))
                .set_auth_uri(
                    AuthUrl::new(self.config.auth_url.clone())
                        .map_err(|_| parse(&self.config.auth_url))?,
                )
                .set_token_uri(
                    TokenUrl::new(self.config.token_url.clone())
                        .map_err(|_| parse(&self.config.token_url))?,
                )
                .set_redirect_uri(
                    RedirectUrl::new(self.config.redirect_url.clone())
                        .map_err(|_| parse(&self.config.redirect_url))?,
                ),
        )
    }

    /// Begin the flow: produce the provider redirect URL and the signed
    /// transaction token to set as a cookie. `jwt` is the app's service; the
    /// transaction inherits its expiry.
    pub fn authorize(&self, jwt: &JwtService) -> Result<Authorization, AuthError> {
        let client = self.basic_client()?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = client.authorize_url(CsrfToken::new_random);
        for scope in &self.config.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, csrf) = request.set_pkce_challenge(challenge).url();
        let transaction = jwt.sign(&Transaction {
            csrf: csrf.secret().clone(),
            pkce: verifier.secret().clone(),
            exp: jwt.expiry(),
        })?;
        Ok(Authorization {
            url: url.to_string(),
            transaction,
        })
    }

    /// Complete the flow: validate the provider's `state` against the signed
    /// `transaction`, then trade `code` for an access token. Returns the raw
    /// access token; fetching userinfo from it is the provider-specific step the
    /// app's strategy performs.
    pub async fn exchange(
        &self,
        jwt: &JwtService,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<String, AuthError> {
        let tx: Transaction = jwt.verify(transaction)?;
        if tx.csrf != state {
            return Err(AuthError::Failed("OAuth state mismatch".into()));
        }
        let token = self
            .basic_client()?
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(tx.pkce))
            .request_async(&self.http)
            .await
            .map_err(|e| AuthError::Failed(e.to_string()))?;
        Ok(token.access_token().secret().clone())
    }

    /// Fetch the caller's profile from the provider's userinfo endpoint with the
    /// access token, deserialized into the app's provider-specific shape. The
    /// app maps that to its own principal — the step a Passport strategy owns.
    pub async fn userinfo<T: DeserializeOwned>(&self, access_token: &str) -> Result<T, AuthError> {
        let body = self
            .http
            .get(&self.config.userinfo_url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| AuthError::Failed(e.to_string()))?
            .error_for_status()
            .map_err(|e| AuthError::Failed(e.to_string()))?
            .text()
            .await
            .map_err(|e| AuthError::Failed(e.to_string()))?;
        serde_json::from_str(&body).map_err(|e| AuthError::Failed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::{JwtOptions, JwtService};

    fn client() -> OAuth2Client {
        OAuth2Client::new(OAuth2Config {
            client_id: "demo-client".into(),
            client_secret: "demo-secret".into(),
            auth_url: "https://provider.example/authorize".into(),
            token_url: "https://provider.example/token".into(),
            redirect_url: "https://app.example/auth/oauth/callback".into(),
            userinfo_url: "https://provider.example/userinfo".into(),
            scopes: vec!["read:user".into()],
        })
        .expect("client builds")
    }

    fn jwt() -> JwtService {
        JwtService::new(JwtOptions::new("test-secret")).expect("HMAC JwtService is infallible")
    }

    #[test]
    fn authorize_url_carries_client_scope_and_pkce_and_a_verifiable_transaction() {
        let jwt = jwt();
        let auth = client().authorize(&jwt).expect("authorize");

        assert!(auth.url.starts_with("https://provider.example/authorize?"));
        assert!(auth.url.contains("client_id=demo-client"));
        assert!(auth.url.contains("scope=read%3Auser"));
        assert!(auth.url.contains("code_challenge="));
        assert!(auth.url.contains("code_challenge_method=S256"));

        // The transaction is a JWT the same service verifies back, and its CSRF
        // matches the `state` the provider echoes in the URL.
        let tx: Transaction = jwt.verify(&auth.transaction).expect("transaction verifies");
        assert!(auth.url.contains(&format!("state={}", tx.csrf)));
        assert!(!tx.pkce.is_empty());
    }

    #[tokio::test]
    async fn exchange_rejects_a_state_that_does_not_match_the_transaction() {
        let jwt = jwt();
        let auth = client().authorize(&jwt).expect("authorize");

        // A forged/incorrect `state` is rejected before any token request runs.
        let err = client()
            .exchange(&jwt, &auth.transaction, "not-the-csrf", "some-code")
            .await
            .expect_err("state mismatch is rejected");
        assert!(matches!(err, AuthError::Failed(_)));
    }
}
