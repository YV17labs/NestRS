//! OAuth2 Authorization Code client (PKCE). Provider endpoints come from [`OAuth2Config`];
//! profile mapping stays in the app's [`Strategy`](crate::passport::Strategy).
//!
//! CSRF `state` and the PKCE verifier ride in a short-lived JWT cookie so the
//! round-trip needs no server-side session storage.

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
use crate::oauth::OAuth2Config;

/// The redirect leg of the flow, produced by [`OAuth2Client::authorize`].
pub struct Authorization {
    pub url: String,
    /// Signed, short-lived token binding the CSRF state to the PKCE verifier.
    /// Set as a cookie on the redirect; pass back to [`exchange`](OAuth2Client::exchange).
    pub transaction: String,
}

/// Carried as a [`JwtService`]-signed cookie so the client cannot forge it.
#[derive(Serialize, Deserialize)]
struct Transaction {
    csrf: String,
    pkce: String,
    exp: u64,
}

pub struct OAuth2Client {
    config: OAuth2Config,
    http: oauth2::reqwest::Client,
}

impl OAuth2Client {
    /// The HTTP backend refuses redirects — following them during a token
    /// exchange is an SSRF risk (per the `oauth2` crate's own guidance).
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

    /// Build the underlying `oauth2` client from a config. Free function (vs.
    /// `&self`) so unit tests can exercise the URL-parse error paths
    /// directly — `Self::new` short-circuits on `validate()` (length ≥ 1)
    /// before the URLs are syntactically checked here.
    pub(crate) fn basic_client(
        config: &OAuth2Config,
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
        Ok(BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_client_secret(ClientSecret::new(config.client_secret.clone()))
            .set_auth_uri(
                AuthUrl::new(config.auth_url.clone()).map_err(|_| parse(&config.auth_url))?,
            )
            .set_token_uri(
                TokenUrl::new(config.token_url.clone()).map_err(|_| parse(&config.token_url))?,
            )
            .set_redirect_uri(
                RedirectUrl::new(config.redirect_url.clone())
                    .map_err(|_| parse(&config.redirect_url))?,
            ))
    }

    /// Lifetime of the signed transaction token and the cookie carrying it.
    /// Short by design: an OAuth handshake completes in seconds, so the
    /// CSRF/PKCE binding must not inherit the full access-token TTL. The
    /// cookie's `Max-Age` and this token `exp` are driven from the same value
    /// so they cannot drift.
    pub const TRANSACTION_TTL_SECS: u64 = 600;

    /// Begin the flow: produce the provider redirect URL and the signed
    /// transaction token to set as a cookie. The transaction lives for
    /// [`Self::TRANSACTION_TTL_SECS`], not the full `JwtService` TTL.
    pub fn authorize(&self, jwt: &JwtService) -> Result<Authorization, AuthError> {
        let client = Self::basic_client(&self.config)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = client.authorize_url(CsrfToken::new_random);
        for scope in &self.config.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, csrf) = request.set_pkce_challenge(challenge).url();
        let transaction = jwt.sign(&Transaction {
            csrf: csrf.secret().clone(),
            pkce: verifier.secret().clone(),
            exp: jwt.expiry_in(Self::TRANSACTION_TTL_SECS),
        })?;
        Ok(Authorization {
            url: url.to_string(),
            transaction,
        })
    }

    /// Complete the flow: validate the provider's `state` against the signed
    /// `transaction`, then trade `code` for an access token. CSRF check runs
    /// before the exchange — never the other way around.
    pub async fn exchange(
        &self,
        jwt: &JwtService,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<String, AuthError> {
        let tx: Transaction = jwt.verify(transaction)?;
        if tx.csrf != state {
            tracing::warn!(
                target: "nest_rs::auth",
                reason = "csrf_state_mismatch",
                "OAuth callback rejected"
            );
            return Err(AuthError::Failed("OAuth state mismatch".into()));
        }
        let token = Self::basic_client(&self.config)?
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(tx.pkce))
            .request_async(&self.http)
            .await
            .map_err(|e| AuthError::Failed(e.to_string()))?;
        Ok(token.access_token().secret().clone())
    }

    /// Fetch the caller's profile, deserialized into the app's
    /// provider-specific shape; mapping it to the app's principal is the
    /// Passport strategy's job.
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
    use crate::jwt::JwtOptions;

    fn valid_config() -> OAuth2Config {
        OAuth2Config {
            client_id: "client".into(),
            client_secret: "secret".into(),
            auth_url: "https://provider.example/authorize".into(),
            token_url: "https://provider.example/token".into(),
            redirect_url: "https://app.example/callback".into(),
            userinfo_url: "https://provider.example/userinfo".into(),
            scopes: vec!["read".into()],
        }
    }

    fn jwt() -> JwtService {
        JwtService::new(JwtOptions::new("oauth-client-tests")).expect("HMAC service")
    }

    #[test]
    fn new_rejects_invalid_config_at_validate_stage() {
        // `OAuth2Config::default()` has empty URL fields → `validate()`
        // (length ≥ 1) trips before any URL is parsed, so `new` surfaces a
        // `Failed`. `OAuth2Client` is not `Debug`, so we test via `is_err`.
        assert!(OAuth2Client::new(OAuth2Config::default()).is_err());
    }

    #[test]
    fn new_accepts_a_valid_config() {
        // Happy `new` path — the URL-parse tests below stand on this baseline.
        assert!(OAuth2Client::new(valid_config()).is_ok());
        OAuth2Client::basic_client(&valid_config()).expect("basic_client builds");
    }

    #[test]
    fn basic_client_rejects_malformed_auth_url() {
        let mut config = valid_config();
        config.auth_url = "not a url".into();
        let Err(AuthError::Failed(msg)) = OAuth2Client::basic_client(&config) else {
            panic!("expected Failed");
        };
        assert!(
            msg.contains("not a url"),
            "error names the offending value: {msg}"
        );
    }

    #[test]
    fn basic_client_rejects_malformed_token_url() {
        let mut config = valid_config();
        config.token_url = "::::".into();
        assert!(matches!(
            OAuth2Client::basic_client(&config),
            Err(AuthError::Failed(_))
        ));
    }

    #[test]
    fn basic_client_rejects_malformed_redirect_url() {
        // A redirect URL must be absolute — a bare path trips `RedirectUrl::new`
        // after auth_url and token_url have parsed successfully.
        let mut config = valid_config();
        config.redirect_url = "/relative/path".into();
        assert!(matches!(
            OAuth2Client::basic_client(&config),
            Err(AuthError::Failed(_))
        ));
    }

    #[test]
    fn authorize_surfaces_basic_client_error() {
        // `validate()` accepts non-empty strings; the URL-syntax check runs
        // inside `basic_client` when `authorize` rebuilds the client. This
        // exercises the `?` propagation path in `authorize`.
        let mut config = valid_config();
        config.auth_url = "not a url".into();
        let client = OAuth2Client::new(config).expect("new accepts non-empty fields");
        assert!(matches!(
            client.authorize(&jwt()),
            Err(AuthError::Failed(_))
        ));
    }

    #[tokio::test]
    async fn exchange_surfaces_url_parse_error_after_csrf_passes() {
        // Forge a transaction whose csrf matches the state we will pass in,
        // so the early `state mismatch` branch is skipped and `exchange`
        // reaches `basic_client(&self.config)?` — which fails on the
        // malformed `token_url` before any network call. Covers the
        // `?` propagation past the CSRF check.
        let jwt = jwt();
        let mut config = valid_config();
        config.token_url = "::::".into();
        let client = OAuth2Client::new(config).expect("new accepts non-empty fields");

        let transaction = jwt
            .sign(&Transaction {
                csrf: "agreed-state".into(),
                pkce: "verifier".into(),
                exp: jwt.expiry(),
            })
            .expect("sign");

        assert!(matches!(
            client
                .exchange(&jwt, &transaction, "agreed-state", "the-code")
                .await,
            Err(AuthError::Failed(_))
        ));
    }
}
