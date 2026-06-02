//! OAuth2 Authorization Code client (PKCE). Provider endpoints come from [`OAuth2Config`];
//! profile mapping stays in the app's [`Strategy`](crate::passport::Strategy).
//!
//! CSRF `state` and the PKCE verifier ride in a short-lived JWT cookie so the
//! round-trip needs no server-side session storage.

use oauth2::basic::BasicClient;
use validator::Validate;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AuthError;
use crate::jwt::JwtService;
use crate::oauth::OAuth2Config;

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
        config
            .validate()
            .map_err(|err| AuthError::Failed(format!("invalid OAuth2 config: {err}")))?;
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
