//! OAuth2 Authorization Code client (PKCE). Provider endpoints come from [`OAuthClientConfig`];
//! profile mapping stays in the app's [`Strategy`](nest_rs_authn::Strategy).
//!
//! CSRF `state` and the PKCE verifier ride in a short-lived JWT cookie so the
//! round-trip needs no server-side session storage.

use std::fmt;
use std::time::Duration;

use oauth2::basic::{BasicClient, BasicErrorResponse};
use oauth2::url::Url;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpClientError,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RequestTokenError, Scope, TokenResponse,
    TokenUrl,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use validator::Validate;

use crate::config::OAuthClientConfig;
use nest_rs_authn::AuthError;
use nest_rs_authn::JwtService;

/// The redirect leg of the flow, produced by [`OAuthClient::authorize`].
pub struct AuthorizationRedirect {
    /// The provider authorization URL to redirect the user agent to.
    pub url: String,
    /// Signed, short-lived token binding the CSRF state to the PKCE verifier.
    /// Set as a cookie on the redirect; pass back to [`exchange`](OAuthClient::exchange).
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
/// The transaction cookie's handshake purpose (RFC 8725 §3.11, explicit typing).
///
/// `JwtService` turns this into a media type in the namespace it reserves for
/// non-access tokens, so a resource server verifying `at+jwt` refuses the
/// cookie and `verify_handshake` refuses an access token replayed as a
/// transaction. The in-payload [`TransactionKind`] discriminator states the
/// same fact where a reader sees it; the header is what a *verifier* acts on.
const TRANSACTION_PURPOSE: &str = "oauth-tx";

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

/// Low-cardinality `reason` codes for the three ways a callback is refused —
/// what an incident query groups on. Constants rather than call-site literals so
/// the set is greppable and a typo cannot silently create a fourth.
const REASON_INVALID_TRANSACTION: &str = "invalid_transaction";
const REASON_PROVIDER_MISMATCH: &str = "provider_mismatch";
const REASON_CSRF_STATE_MISMATCH: &str = "csrf_state_mismatch";

/// The one message every callback refusal is filed under, so an operator greps
/// once and reads `reason` to tell the three apart.
const CALLBACK_REJECTED: &str = "OAuth callback rejected";

/// RFC 6749 §3.2's name for where the code is traded for a token.
const TOKEN_ENDPOINT: &str = "token endpoint";
/// OpenID Connect Core §5.3's name for where the caller's profile is read.
const USERINFO_ENDPOINT: &str = "userinfo endpoint";
/// Any other endpoint a provider reads through [`OAuthClient::fetch`] — GitHub's
/// verified emails, say — which no standard names.
const PROVIDER_ENDPOINT: &str = "endpoint";

/// A provider endpoint as the sentence of a failed call names it: which endpoint,
/// and where.
///
/// The address is the URL's origin and path and nothing else. A query and a
/// userinfo part are where a deployment would put an API key, and this sentence
/// reaches every line that reports the failure. The call's own secrets — the
/// client secret and the code of an exchange, the access token of a read — travel
/// in its body and headers, so an address cannot quote them either.
struct Endpoint<'a> {
    role: &'static str,
    url: &'a str,
}

impl fmt::Display for Endpoint<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the OAuth provider's {} (", self.role)?;
        match Url::parse(self.url) {
            Ok(url) => write!(f, "{}{}", url.origin().ascii_serialization(), url.path())?,
            Err(_) => f.write_str("an address that does not parse")?,
        }
        f.write_str(")")
    }
}

/// What a call to `endpoint` that got no answer is reported as: an
/// [`AuthError::Unavailable`] — the provider could not be reached or did not
/// answer in time, which the caller did nothing to cause and can only wait out
/// (RFC 9110 §15.6.4) — whose sentence names the endpoint and, when one of this
/// client's bounds ended the call, which bound. It was a `Failed`, answered
/// `401`, which told the person signing in that their sign-in was wrong.
///
/// The rest of the cause is reqwest's own chain with the URL taken out, since
/// reqwest quotes the URL whole, query included.
fn call_failed(endpoint: &Endpoint<'_>, error: reqwest::Error) -> AuthError {
    let detail = if error.is_timeout() && error.is_connect() {
        format!(
            "{endpoint} could not be connected to within {:?}",
            OAuthClient::CONNECT_TIMEOUT
        )
    } else if error.is_timeout() {
        format!(
            "{endpoint} did not answer within {:?}",
            OAuthClient::CALL_TIMEOUT
        )
    } else {
        format!(
            "{endpoint} could not be called: {}",
            nest_rs_core::error_message(&error.without_url())
        )
    };
    AuthError::Unavailable {
        detail,
        retry_after: None,
    }
}

/// What a provider's answer with `status` says about the provider, if it says
/// the provider cannot answer now: a status of the 5xx class, or `429`
/// (RFC 6585 §4), is the provider's state rather than the caller's mistake, so
/// it is an [`AuthError::Unavailable`] carrying the provider's own `Retry-After`
/// when it gave one in delay-seconds. An HTTP-date `Retry-After` is not read:
/// converting it means trusting the provider's clock against this one, and the
/// caller is still told to come back, without a figure.
fn provider_unavailable(
    endpoint: &Endpoint<'_>,
    status: u16,
    retry_after: Option<&[u8]>,
) -> Option<AuthError> {
    (status == 429 || (500..600).contains(&status)).then(|| AuthError::Unavailable {
        detail: format!("{endpoint} answered {status}"),
        retry_after: retry_after
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs),
    })
}

/// A code exchange that failed: [`provider_unavailable`] when the provider
/// answered that it cannot answer now, [`call_failed`] when it was never heard
/// from, and otherwise the provider's own answer — RFC 6749 §5.2's error, or
/// why its body did not parse — after the endpoint that gave it.
fn exchange_failed(
    endpoint: &Endpoint<'_>,
    answered: Option<Answered>,
    error: RequestTokenError<HttpClientError<reqwest::Error>, BasicErrorResponse>,
) -> AuthError {
    if let Some(unavailable) = answered.and_then(|answered| {
        provider_unavailable(endpoint, answered.status, answered.retry_after.as_deref())
    }) {
        return unavailable;
    }
    match error {
        RequestTokenError::Request(HttpClientError::Reqwest(error)) => {
            call_failed(endpoint, *error)
        }
        other => AuthError::Failed(format!("{endpoint}: {other}")),
    }
}

/// The status and the `Retry-After` of the token endpoint's answer, read off the
/// response before `oauth2` turns it into an error that keeps neither.
struct Answered {
    status: u16,
    retry_after: Option<Vec<u8>>,
}

/// The client's HTTP backend, keeping the last answer's [`Answered`] — what the
/// exchange hands `oauth2` in place of the backend itself.
struct Observed<'a> {
    http: &'a reqwest::Client,
    answered: std::sync::Mutex<Option<Answered>>,
}

impl<'c> oauth2::AsyncHttpClient<'c> for Observed<'_> {
    type Error = HttpClientError<reqwest::Error>;
    type Future = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<oauth2::HttpResponse, Self::Error>> + Send + 'c,
        >,
    >;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let request = reqwest::Request::try_from(request).map_err(Box::new)?;
            let response = self.http.execute(request).await.map_err(Box::new)?;
            if let Ok(mut slot) = self.answered.lock() {
                *slot = Some(Answered {
                    status: response.status().as_u16(),
                    retry_after: response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .map(|value| value.as_bytes().to_vec()),
                });
            }
            let mut answer = oauth2::http::Response::builder()
                .status(response.status())
                .version(response.version());
            for (name, value) in response.headers() {
                answer = answer.header(name, value);
            }
            let body = response.bytes().await.map_err(Box::new)?;
            answer.body(body.to_vec()).map_err(HttpClientError::Http)
        })
    }
}

/// The HTTP backend every call to a provider goes through, as
/// [`OAuthClient::new`] builds it: no redirect followed, one user-agent, and
/// both bounds.
fn backend() -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        // A client-wide UA so every outbound request carries it uniformly —
        // some provider APIs (GitHub) reject requests without one, and the
        // token exchange uses this same client.
        .user_agent("nestrs")
        .connect_timeout(OAuthClient::CONNECT_TIMEOUT)
        .timeout(OAuthClient::CALL_TIMEOUT)
}

/// A transient Authorization-Code (PKCE) client built per flow from an
/// [`OAuthClientConfig`]. Its HTTP backend refuses redirects (anti-SSRF), carries
/// a fixed user-agent, and bounds every call it makes; see [`new`](Self::new).
pub struct OAuthClient {
    config: OAuthClientConfig,
    http: reqwest::Client,
}

impl OAuthClient {
    /// How long reaching a provider may take — resolving its name, the TCP
    /// handshake and the TLS one — before a call gives up, naming the endpoint.
    ///
    /// A provider's endpoints are public APIs a healthy network reaches in tens
    /// of milliseconds, and in about a second when a lost SYN has to be sent
    /// again. Three seconds covers that retransmission and the handshakes after
    /// it; a connection that needs longer is a network that is not delivering,
    /// and saying so then keeps the rest of [`CALL_TIMEOUT`](Self::CALL_TIMEOUT)
    /// from being spent on it.
    pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

    /// How long one call to a provider may take in all — connecting, sending,
    /// and reading the whole answer — before it fails with a sentence naming the
    /// endpoint and this bound.
    ///
    /// The calls are the code exchange at the token endpoint and the reads that
    /// follow it: the userinfo endpoint, and any second read a provider makes
    /// through [`fetch`](Self::fetch), as GitHub's verified emails are. An
    /// identity provider answers each of them in well under a second; five
    /// seconds is several times a slow answer, and past it the person waiting on
    /// the callback is better served by a failure they can retry than by a page
    /// that hangs.
    ///
    /// **Below the nets above it, for a whole login.** A callback makes these
    /// calls one after another — three for GitHub, the most any provider
    /// `nest-rs-social` ships makes — so a login spends at most three times this
    /// bound on its provider, 15 s. That stays under the
    /// [`AuthnGuard`](nest_rs_authn::AuthnGuard)'s
    /// [`AUTHENTICATE_TIMEOUT`](nest_rs_authn::AUTHENTICATE_TIMEOUT) (20 s) and
    /// the HTTP edge's request timeout (`<PREFIX>_HTTP__REQUEST_TIMEOUT_SECS`,
    /// 30 s by default), which must stay the larger: a provider that stops
    /// answering is then reported here, naming its endpoint, before either net
    /// replaces that with a sentence naming nothing but a strategy or a route.
    pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

    /// The HTTP backend refuses redirects — following them during a token
    /// exchange is an SSRF risk (per the `oauth2` crate's own guidance) — and
    /// bounds every call at [`CONNECT_TIMEOUT`](Self::CONNECT_TIMEOUT) and
    /// [`CALL_TIMEOUT`](Self::CALL_TIMEOUT), so a provider that never answers
    /// fails the call instead of holding it.
    pub fn new(config: OAuthClientConfig) -> Result<Self, AuthError> {
        config
            .validate()
            .map_err(|err| AuthError::Failed(format!("invalid OAuth2 config: {err}")))?;
        let http = backend()
            .build()
            .map_err(|e| AuthError::Failed(e.to_string()))?;
        Ok(Self { config, http })
    }

    /// Build the underlying `oauth2` client from a config. Free function (vs.
    /// `&self`) so unit tests can exercise the URL-parse error paths
    /// directly — `Self::new` short-circuits on `validate()` (length ≥ 1)
    /// before the URLs are syntactically checked here.
    pub(crate) fn basic_client(
        config: &OAuthClientConfig,
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
        let parse = |s: &str, error: oauth2::url::ParseError| {
            AuthError::Failed(format!("invalid OAuth URL: {s}: {error}"))
        };
        Ok(BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_client_secret(ClientSecret::new(config.client_secret.clone()))
            .set_auth_uri(
                AuthUrl::new(config.auth_url.clone()).map_err(|e| parse(&config.auth_url, e))?,
            )
            .set_token_uri(
                TokenUrl::new(config.token_url.clone()).map_err(|e| parse(&config.token_url, e))?,
            )
            .set_redirect_uri(
                RedirectUrl::new(config.redirect_url.clone())
                    .map_err(|e| parse(&config.redirect_url, e))?,
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
    pub fn authorize(
        &self,
        jwt: &JwtService,
        provider: &str,
    ) -> Result<AuthorizationRedirect, AuthError> {
        let client = Self::basic_client(&self.config)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = client.authorize_url(CsrfToken::new_random);
        for scope in &self.config.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, csrf) = request.set_pkce_challenge(challenge).url();
        let transaction = jwt.sign_handshake(
            TRANSACTION_PURPOSE,
            &Transaction {
                typ: TransactionKind::OauthTx,
                provider: provider.to_owned(),
                csrf: csrf.secret().clone(),
                pkce: verifier.secret().clone(),
                exp: jwt.expiry_in(Self::TRANSACTION_TTL_SECS),
            },
        )?;
        Ok(AuthorizationRedirect {
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
        // A transaction cookie that does not verify is a forged or replayed
        // handshake, and it is this crate's third way to refuse a callback — so
        // it files the same `warn` its two siblings below do. Nothing else
        // would: `JwtService` keeps its typed decode reason at `debug` because
        // on the *strategy* path `AuthnGuard` emits the single `warn`, and this
        // path has no guard above it — the error `?`s out through
        // `AuthError::render`, which logs only `Failed`/`Unavailable`. So a
        // forged cookie left no `warn` anywhere. The `debug` stays where it is;
        // one event per refusal, said at the site that knows what was refused.
        let tx: Transaction = jwt
            .verify_handshake(TRANSACTION_PURPOSE, transaction)
            .inspect_err(|error| {
                tracing::warn!(
                target: crate::TARGET,
                reason = REASON_INVALID_TRANSACTION,
                token_reason = error.reason(),
                provider,
                "{CALLBACK_REJECTED}",
                );
            })?;
        // Provider binding first: a transaction replayed on another provider's
        // callback is rejected before its CSRF value is ever compared.
        if tx.provider != provider {
            tracing::warn!(
                target: crate::TARGET,
                reason = REASON_PROVIDER_MISMATCH,
                expected = provider,
                "{CALLBACK_REJECTED}",
            );
            return Err(AuthError::Failed("OAuth provider mismatch".into()));
        }
        // Constant-time compare (mirrors the client-credentials check); a length
        // mismatch reads as "not equal" via `subtle`'s slice `ct_eq`.
        if !bool::from(tx.csrf.as_bytes().ct_eq(state.as_bytes())) {
            tracing::warn!(
                target: crate::TARGET,
                reason = REASON_CSRF_STATE_MISMATCH,
                provider,
                "{CALLBACK_REJECTED}",
            );
            return Err(AuthError::Failed("OAuth state mismatch".into()));
        }
        let endpoint = Endpoint {
            role: TOKEN_ENDPOINT,
            url: &self.config.token_url,
        };
        // The answer's status is read on the way through, because `oauth2`
        // drops it: a provider's `503` page parses as no §5.2 error, and would
        // be reported as the caller's failure rather than the provider's.
        let observed = Observed {
            http: &self.http,
            answered: std::sync::Mutex::new(None),
        };
        let exchanged = Self::basic_client(&self.config)?
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(tx.pkce))
            .request_async(&observed)
            .await;
        let token = exchanged.map_err(|error| {
            let answered = observed
                .answered
                .lock()
                .ok()
                .and_then(|mut slot| slot.take());
            exchange_failed(&endpoint, answered, error)
        })?;
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
    /// built in [`new`](Self::new), and its bounds, instead of standing up its
    /// own reqwest.
    pub async fn fetch<T: DeserializeOwned>(
        &self,
        url: &str,
        access_token: &str,
    ) -> Result<T, AuthError> {
        let endpoint = Endpoint {
            role: PROVIDER_ENDPOINT,
            url,
        };
        self.read(&endpoint, access_token).await
    }

    /// Fetch the caller's profile from the configured `userinfo_url`,
    /// deserialized into the app's provider-specific shape; mapping it to the
    /// app's principal is the Passport strategy's job.
    pub async fn userinfo<T: DeserializeOwned>(&self, access_token: &str) -> Result<T, AuthError> {
        let endpoint = Endpoint {
            role: USERINFO_ENDPOINT,
            url: &self.config.userinfo_url,
        };
        self.read(&endpoint, access_token).await
    }

    /// The authenticated `GET` both reads make. Anything but a `2xx` is a
    /// failure — a `3xx` included, since the client follows no redirect — and
    /// every failure names the endpoint rather than quoting its URL: the
    /// provider's own unavailability ([`provider_unavailable`]) as such, any
    /// other status as the token's refusal.
    async fn read<T: DeserializeOwned>(
        &self,
        endpoint: &Endpoint<'_>,
        access_token: &str,
    ) -> Result<T, AuthError> {
        let response = self
            .http
            .get(endpoint.url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| call_failed(endpoint, error))?;
        let status = response.status();
        if !status.is_success() {
            return Err(provider_unavailable(
                endpoint,
                status.as_u16(),
                response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .map(|value| value.as_bytes()),
            )
            .unwrap_or_else(|| AuthError::Failed(format!("{endpoint} answered {status}"))));
        }
        let body = response
            .text()
            .await
            .map_err(|error| call_failed(endpoint, error))?;
        // Said without the value serde would quote: a provider's body carries
        // the profile, and may carry a token.
        serde_json::from_str(&body).map_err(|error| {
            AuthError::Failed(format!(
                "{endpoint} answered a body that does not parse: {}",
                nest_rs_core::DecodeError::new(&error),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_authn::JwtOptions;

    fn valid_config() -> OAuthClientConfig {
        OAuthClientConfig {
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
        // `OAuthClientConfig::default()` has empty URL fields → `validate()`
        // (length ≥ 1) trips before any URL is parsed, so `new` surfaces a
        // `Failed`. `OAuthClient` is not `Debug`, so we test via `is_err`.
        assert!(OAuthClient::new(OAuthClientConfig::default()).is_err());
    }

    #[test]
    fn new_accepts_a_valid_config() {
        // Happy `new` path — the URL-parse tests below stand on this baseline.
        assert!(OAuthClient::new(valid_config()).is_ok());
        OAuthClient::basic_client(&valid_config()).expect("basic_client builds");
    }

    #[test]
    fn basic_client_rejects_malformed_auth_url() {
        let mut config = valid_config();
        config.auth_url = "not a url".into();
        let Err(AuthError::Failed(msg)) = OAuthClient::basic_client(&config) else {
            panic!("expected Failed");
        };
        assert!(
            msg.contains("not a url"),
            "error names the offending value: {msg}"
        );
        assert!(
            msg.contains("relative URL without a base"),
            "and the parser's reason, which the value alone does not say: {msg}"
        );
    }

    #[test]
    fn basic_client_rejects_malformed_token_url() {
        let mut config = valid_config();
        config.token_url = "::::".into();
        assert!(matches!(
            OAuthClient::basic_client(&config),
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
            OAuthClient::basic_client(&config),
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
        let client = OAuthClient::new(config).expect("new accepts non-empty fields");
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
        let client = OAuthClient::new(config).expect("new accepts non-empty fields");

        let transaction = jwt
            .sign_handshake(
                TRANSACTION_PURPOSE,
                &Transaction {
                    typ: TransactionKind::OauthTx,
                    provider: "acme".into(),
                    csrf: "agreed-state".into(),
                    pkce: "verifier".into(),
                    exp: jwt.expiry(),
                },
            )
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
        let client = OAuthClient::new(valid_config()).expect("new accepts a valid config");
        let transaction = jwt
            .sign_handshake(
                TRANSACTION_PURPOSE,
                &Transaction {
                    typ: TransactionKind::OauthTx,
                    provider: "provider-a".into(),
                    csrf: "agreed-state".into(),
                    pkce: "verifier".into(),
                    exp: jwt.expiry(),
                },
            )
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
        let client = OAuthClient::new(valid_config()).expect("new accepts a valid config");
        let transaction = jwt
            .sign_handshake(
                TRANSACTION_PURPOSE,
                &Transaction {
                    typ: TransactionKind::OauthTx,
                    provider: "acme".into(),
                    csrf: "the-signed-state".into(),
                    pkce: "verifier".into(),
                    exp: jwt.expiry(),
                },
            )
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

    /// A name lookup that never answers — the first thing a call waits on, and
    /// inside the connect bound, like the handshakes after it.
    struct NeverResolves;

    impl reqwest::dns::Resolve for NeverResolves {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(std::future::pending())
        }
    }

    /// A provider that cannot be reached is told apart from one that does not
    /// answer: the call fails at `CONNECT_TIMEOUT`, sooner than the whole call's
    /// bound, and says which bound it was. Driven on the backend `new` builds,
    /// with only its resolver swapped for one that never answers — a connection
    /// no local listener can be made to leave hanging, since the kernel
    /// completes a handshake to any listening socket and resets one it cannot
    /// queue.
    #[tokio::test(start_paused = true)]
    async fn a_provider_that_cannot_be_reached_fails_at_the_connect_timeout() {
        let client = OAuthClient {
            config: OAuthClientConfig {
                userinfo_url: "https://provider.example/userinfo?key=never-quoted".into(),
                ..valid_config()
            },
            http: backend()
                .dns_resolver(std::sync::Arc::new(NeverResolves))
                .build()
                .expect("the backend builds"),
        };

        // Twice the bound and no longer: a backend that stopped bounding its
        // connections would otherwise wait on this resolver for good.
        let sent = tokio::time::Instant::now();
        let Err(AuthError::Unavailable {
            detail: sentence,
            retry_after: None,
        }) = tokio::time::timeout(
            OAuthClient::CONNECT_TIMEOUT * 2,
            client.userinfo::<serde_json::Value>("never-quoted"),
        )
        .await
        .expect("no answer within twice CONNECT_TIMEOUT")
        else {
            panic!("a provider nobody can reach is unavailable, and returns no profile");
        };

        assert_eq!(sent.elapsed(), OAuthClient::CONNECT_TIMEOUT);
        assert_eq!(
            sentence,
            format!(
                "the OAuth provider's userinfo endpoint (https://provider.example/userinfo) \
                 could not be connected to within {:?}",
                OAuthClient::CONNECT_TIMEOUT
            ),
        );
    }
}
