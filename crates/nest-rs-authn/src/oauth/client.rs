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
use subtle::ConstantTimeEq;
use validator::Validate;

use crate::error::AuthError;
use crate::jwt::JwtService;
use crate::oauth::OAuth2Config;

/// The redirect leg of the flow, produced by [`OAuth2Client::authorize`].
pub struct Authorization {
    /// The provider authorization URL to redirect the user agent to.
    pub url: String,
    /// Signed, short-lived token binding the CSRF state to the PKCE verifier.
    /// Set as a cookie on the redirect; pass back to [`exchange`](OAuth2Client::exchange).
    pub transaction: String,
}

/// Outcome of the Authorization-Code exchange.
///
/// `#[non_exhaustive]`: downstream provider crates (`nest-rs-social` and
/// third-party providers) match on these fields, so adding one later — a new
/// standard token field — must not break them. Construct via the field
/// initializers inside this crate only; consumers read.
///
/// The base flow populates `access_token` (and `refresh_token` when the
/// provider returns one). `id_token` stays `None` for the standard resource
/// path: an OIDC provider that reads identity from the id_token overrides
/// `SocialProvider::exchange` (e.g. Apple, which has no userinfo endpoint) and
/// fills it there — the base client does not parse OIDC extra fields.
#[non_exhaustive]
pub struct TokenSet {
    /// The bearer access token used to call the provider's APIs (e.g. userinfo).
    pub access_token: String,
    /// The OIDC id_token, when the provider reads identity from it. `None` on
    /// the standard resource path — an OIDC provider fills it by overriding
    /// `SocialProvider::exchange`.
    pub id_token: Option<String>,
    /// The refresh token, when the provider issued one; `None` otherwise.
    pub refresh_token: Option<String>,
}

/// Token-kind discriminant. Any other `typ` fails to deserialize, so a token
/// minted for a different purpose by the same [`JwtService`] (an access token,
/// say) can never be replayed as a transaction.
#[derive(Serialize, Deserialize)]
enum TransactionKind {
    #[serde(rename = "oauth_tx")]
    OauthTx,
}

/// Carried as a [`JwtService`]-signed cookie so the client cannot forge it.
///
/// `provider` binds the transaction to the flow that minted it. Apps store the
/// cookie under a single name for every provider (the reference app does), so
/// without that binding a transaction obtained from provider A would verify on
/// provider B's callback — a code/login-confusion class that only PKCE would
/// still stand in the way of, and PKCE is not mandatory for confidential
/// clients at several real providers.
#[derive(Serialize, Deserialize)]
struct Transaction {
    typ: TransactionKind,
    provider: String,
    csrf: String,
    pkce: String,
    exp: u64,
}

/// A transient Authorization-Code (PKCE) client built per flow from an
/// [`OAuth2Config`]. Its HTTP backend refuses redirects (anti-SSRF) and carries
/// a fixed user-agent; see [`new`](Self::new).
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
            // A client-wide UA so every outbound request carries it uniformly —
            // some provider APIs (GitHub) reject requests without one, and the
            // token exchange uses this same client.
            .user_agent("nestrs")
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
    ///
    /// `provider` is the key this transaction is bound to; [`exchange`](Self::exchange)
    /// refuses one minted for a different provider.
    pub fn authorize(&self, jwt: &JwtService, provider: &str) -> Result<Authorization, AuthError> {
        let client = Self::basic_client(&self.config)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = client.authorize_url(CsrfToken::new_random);
        for scope in &self.config.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, csrf) = request.set_pkce_challenge(challenge).url();
        let transaction = jwt.sign(&Transaction {
            typ: TransactionKind::OauthTx,
            provider: provider.to_owned(),
            csrf: csrf.secret().clone(),
            pkce: verifier.secret().clone(),
            exp: jwt.expiry_in(Self::TRANSACTION_TTL_SECS),
        })?;
        Ok(Authorization {
            url: url.to_string(),
            transaction,
        })
    }

    /// Complete the flow: check the signed `transaction` belongs to `provider`,
    /// validate the provider's `state` against it, then trade `code` for a
    /// [`TokenSet`]. Both checks run before the exchange — never the other way
    /// around.
    pub async fn exchange(
        &self,
        jwt: &JwtService,
        provider: &str,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<TokenSet, AuthError> {
        let tx: Transaction = jwt.verify(transaction)?;
        // Provider binding first: a transaction replayed on another provider's
        // callback is rejected before its CSRF value is ever compared.
        if tx.provider != provider {
            tracing::warn!(
                target: "nest_rs::authn",
                reason = "provider_mismatch",
                expected = provider,
                "OAuth callback rejected",
            );
            return Err(AuthError::Failed("OAuth provider mismatch".into()));
        }
        // Constant-time compare (mirrors the client-credentials check); a length
        // mismatch reads as "not equal" via `subtle`'s slice `ct_eq`.
        if !bool::from(tx.csrf.as_bytes().ct_eq(state.as_bytes())) {
            tracing::warn!(
                target: "nest_rs::authn",
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
        Ok(TokenSet {
            access_token: token.access_token().secret().clone(),
            id_token: None,
            refresh_token: token.refresh_token().map(|t| t.secret().clone()),
        })
    }

    /// Authenticated `GET` against an arbitrary provider endpoint, deserialized
    /// into `T`. The generalization of [`userinfo`](Self::userinfo): a provider
    /// whose profile needs a second call (GitHub's verified-emails endpoint)
    /// reuses this so it inherits the redirect-refusing, anti-SSRF HTTP client
    /// built in [`new`](Self::new) instead of standing up its own reqwest.
    pub async fn fetch<T: DeserializeOwned>(
        &self,
        url: &str,
        access_token: &str,
    ) -> Result<T, AuthError> {
        let body = self
            .http
            .get(url)
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

    /// Fetch the caller's profile from the configured `userinfo_url`,
    /// deserialized into the app's provider-specific shape; mapping it to the
    /// app's principal is the Passport strategy's job.
    pub async fn userinfo<T: DeserializeOwned>(&self, access_token: &str) -> Result<T, AuthError> {
        self.fetch(&self.config.userinfo_url, access_token).await
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
        JwtService::new(JwtOptions::new("oauth-client-tests-padded-to-32b")).expect("HMAC service")
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
            client.authorize(&jwt(), "acme"),
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
                typ: TransactionKind::OauthTx,
                provider: "acme".into(),
                csrf: "agreed-state".into(),
                pkce: "verifier".into(),
                exp: jwt.expiry(),
            })
            .expect("sign");

        assert!(matches!(
            client
                .exchange(&jwt, "acme", &transaction, "agreed-state", "the-code")
                .await,
            Err(AuthError::Failed(_))
        ));
    }

    #[tokio::test]
    async fn exchange_rejects_a_transaction_minted_for_another_provider() {
        // Apps carry the transaction in one cookie for every provider, so this
        // binding — not PKCE — is what keeps provider A's handshake from being
        // completed on provider B's callback. Checked before the CSRF compare,
        // so a matching state does not help the attacker.
        let jwt = jwt();
        let client = OAuth2Client::new(valid_config()).expect("new accepts a valid config");
        let transaction = jwt
            .sign(&Transaction {
                typ: TransactionKind::OauthTx,
                provider: "provider-a".into(),
                csrf: "agreed-state".into(),
                pkce: "verifier".into(),
                exp: jwt.expiry(),
            })
            .expect("sign");

        let Err(err) = client
            .exchange(&jwt, "provider-b", &transaction, "agreed-state", "code")
            .await
        else {
            panic!("a transaction from another provider is rejected");
        };
        assert!(err.to_string().contains("provider mismatch"), "{err}");
    }

    #[tokio::test]
    async fn exchange_rejects_a_state_that_does_not_match() {
        // A `state` differing from the signed transaction's csrf — here also of
        // a different length — is rejected by the constant-time compare before
        // any code exchange, so no network call is reached.
        let jwt = jwt();
        let client = OAuth2Client::new(valid_config()).expect("new accepts a valid config");
        let transaction = jwt
            .sign(&Transaction {
                typ: TransactionKind::OauthTx,
                provider: "acme".into(),
                csrf: "the-signed-state".into(),
                pkce: "verifier".into(),
                exp: jwt.expiry(),
            })
            .expect("sign");

        // `TokenSet` is intentionally not `Debug` (it carries tokens), so match
        // rather than `expect_err`.
        let Err(err) = client
            .exchange(&jwt, "acme", &transaction, "forged", "the-code")
            .await
        else {
            panic!("a mismatched state is rejected");
        };
        assert!(matches!(err, AuthError::Failed(_)));
        assert!(err.to_string().contains("state mismatch"));
    }
}
