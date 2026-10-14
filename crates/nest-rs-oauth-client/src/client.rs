//! OAuth2 Authorization Code client (PKCE). Provider endpoints come from [`OAuthClientConfig`];
//! profile mapping stays in the app's [`Strategy`](nest_rs_authn::Strategy).

use std::fmt;
use std::sync::PoisonError;
use std::time::Duration;

use aws_lc_rs::constant_time::verify_slices_are_equal;
use nest_rs_config::{ClientTls, ConfigError, ConfigService, Namespaced};
use oauth2::basic::{BasicClient, BasicErrorResponse};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpClientError,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RequestTokenError, Scope, TokenResponse,
    TokenUrl,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

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
#[non_exhaustive]
pub struct TokenSet {
    /// The bearer access token used to call the provider's APIs (e.g. userinfo).
    pub access_token: String,
    /// The OIDC id_token; `None` unless an OIDC provider fills it by overriding
    /// `SocialProvider::exchange`.
    pub id_token: Option<String>,
    /// The refresh token, when the provider issued one; `None` otherwise.
    pub refresh_token: Option<String>,
}

/// The transaction cookie's handshake purpose (RFC 8725 §3.11): a resource server
/// verifying `at+jwt` refuses the cookie, and an access token is refused as one.
const TRANSACTION_PURPOSE: &str = "oauth-tx";

#[derive(Serialize, Deserialize)]
enum TransactionKind {
    #[serde(rename = "oauth_tx")]
    OauthTx,
}

/// Carried as a [`JwtService`]-signed cookie so the client cannot forge it.
/// `provider` binds it to the flow that minted it: apps keep one cookie name for
/// every provider, and PKCE alone would not stop a cross-provider replay.
#[derive(Serialize, Deserialize)]
struct Transaction {
    typ: TransactionKind,
    provider: String,
    csrf: String,
    pkce: String,
    exp: u64,
}

/// Low-cardinality `reason` codes for the three ways a callback is refused.
const REASON_INVALID_TRANSACTION: &str = "invalid_transaction";
const REASON_PROVIDER_MISMATCH: &str = "provider_mismatch";
const REASON_CSRF_STATE_MISMATCH: &str = "csrf_state_mismatch";

/// The one message every callback refusal is filed under; `reason` tells them apart.
const CALLBACK_REJECTED: &str = "OAuth callback rejected";

/// RFC 6749 §3.2's name for where the code is traded for a token.
const TOKEN_ENDPOINT: &str = "token endpoint";
/// OpenID Connect Core §5.3's name for where the caller's profile is read.
const USERINFO_ENDPOINT: &str = "userinfo endpoint";
/// Any other endpoint a provider reads through [`OAuthClient::fetch`] — GitHub's
/// verified emails, say — which no standard names.
const PROVIDER_ENDPOINT: &str = "endpoint";

/// A provider endpoint as a failed call's sentence names it, through
/// [`endpoint_shown`](nest_rs_config::endpoint_shown): a query or userinfo part
/// may carry an API key, and the sentence is logged.
struct Endpoint<'a> {
    role: &'static str,
    url: &'a str,
}

impl fmt::Display for Endpoint<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the OAuth provider's {} ({})",
            self.role,
            nest_rs_config::endpoint_shown(self.url)
        )
    }
}

/// A call to `endpoint` that got no answer, as [`AuthError::Unavailable`] (RFC 9110
/// §15.6.4); reqwest's chain goes in without the URL, which it quotes whole.
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

/// A 5xx or `429` (RFC 6585 §4) answer as [`AuthError::Unavailable`], carrying a
/// delay-seconds `Retry-After`; an HTTP-date one would trust the provider's clock.
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
            *self.answered.lock().unwrap_or_else(PoisonError::into_inner) = Some(Answered {
                status: response.status().as_u16(),
                retry_after: response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .map(|value| value.as_bytes().to_vec()),
            });
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

/// `validator`'s own `Display` lists each rule's parameters, the rejected value
/// among them; [`ConfigError::validation`] says the field and the rule alone.
fn invalid(errors: ValidationErrors) -> AuthError {
    AuthError::Failed(ConfigError::validation(OAuthClientConfig::NAMESPACE, errors).to_string())
}

/// Every call carries a code, a client secret or a bearer token (RFC 6749 §3.1,
/// §3.2; RFC 6750 §5.3), so the backend dials `https` alone, whatever URL a
/// provider builds at run time.
fn backend() -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        // Some provider APIs (GitHub) reject a request without a user-agent.
        .user_agent("nestrs")
        .connect_timeout(OAuthClient::CONNECT_TIMEOUT)
        .timeout(OAuthClient::CALL_TIMEOUT)
}

/// The backend trusting what `tls` names — the system's authorities unless it
/// names others — and presenting its certificate when it holds one.
fn verified(tls: &ClientTls) -> Result<reqwest::Client, AuthError> {
    let names = ConfigService::for_namespace(OAuthClientConfig::NAMESPACE);
    tls.check(&names)
        .map_err(|refused| AuthError::Failed(refused.to_string()))?;
    let roots = reqwest::Certificate::from_pem_bundle(tls.authorities_pem()).map_err(|error| {
        AuthError::Failed(format!(
            "the authorities the OAuth provider's certificate chains to do not read as PEM: {}",
            nest_rs_core::error_message(&error)
        ))
    })?;
    let mut builder = backend().tls_certs_only(roots);
    if let Some((cert, key)) = tls.identity_pem() {
        let pem = [cert, b"\n", key].concat();
        let identity = reqwest::Identity::from_pem(&pem).map_err(|error| {
            AuthError::Failed(format!(
                "{} and {} could not be presented: {}",
                names.spellings("TLS_CERT"),
                names.spellings("TLS_KEY"),
                nest_rs_core::error_message(&error)
            ))
        })?;
        builder = builder.identity(identity);
    }
    builder.build().map_err(|error| {
        AuthError::Failed(format!(
            "the OAuth client could not be built: {}",
            nest_rs_core::error_message(&error)
        ))
    })
}

/// A transient Authorization-Code (PKCE) client built per flow from an
/// [`OAuthClientConfig`]; its backend refuses redirects (anti-SSRF) and bounds every call.
pub struct OAuthClient {
    config: OAuthClientConfig,
    http: reqwest::Client,
}

impl OAuthClient {
    /// How long reaching a provider (DNS, TCP and TLS handshakes) may take before a
    /// call gives up, naming the endpoint.
    pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

    /// How long one call to a provider may take in all before it fails, naming the
    /// endpoint and this bound.
    ///
    /// A login makes up to three calls (GitHub), 15 s, which must stay under
    /// [`AUTHENTICATE_TIMEOUT`](nest_rs_authn::AUTHENTICATE_TIMEOUT) (18 s) and the
    /// HTTP edge's request timeout, or a silent provider is reported by neither name.
    pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

    /// Builds a client from a validated `config`; its backend follows no redirect
    /// (an SSRF risk in a token exchange, per `oauth2`), bounds every call and
    /// trusts what `config.tls` names. A provider endpoint that is not `https`,
    /// a redirect that is neither `https` nor loopback `http`, and TLS material
    /// no handshake could use are refused, naming the setting.
    pub fn new(config: OAuthClientConfig) -> Result<Self, AuthError> {
        config.validate().map_err(invalid)?;
        config.refuse_plaintext()?;
        let http = verified(&config.tls)?;
        Ok(Self { config, http })
    }

    /// Builds the `oauth2` client; URL syntax is checked here, not by `validate()`.
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
        // `url::ParseError` says why in a fixed sentence; the URL itself may
        // carry userinfo, and the sentence is logged.
        let parse = |field: &'static str| {
            move |error: oauth2::url::ParseError| {
                AuthError::Failed(format!(
                    "OAuthClientConfig::{field} does not parse as a URL: {error}"
                ))
            }
        };
        Ok(BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_client_secret(ClientSecret::new(config.client_secret.clone()))
            .set_auth_uri(AuthUrl::new(config.auth_url.clone()).map_err(parse("auth_url"))?)
            .set_token_uri(TokenUrl::new(config.token_url.clone()).map_err(parse("token_url"))?)
            .set_redirect_uri(
                RedirectUrl::new(config.redirect_url.clone()).map_err(parse("redirect_url"))?,
            ))
    }

    /// Lifetime of the signed transaction token and of its cookie's `Max-Age`.
    pub const TRANSACTION_TTL_SECS: u64 = 600;

    /// Begin the flow: the provider redirect URL and the signed transaction token to
    /// set as a cookie, bound to `provider`.
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

    /// Complete the flow: check `transaction` belongs to `provider` and matches
    /// `state`, both before trading `code` for a [`TokenSet`].
    pub async fn exchange(
        &self,
        jwt: &JwtService,
        provider: &str,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<TokenSet, AuthError> {
        // The only `warn` for a forged cookie: no guard sits above this path, and
        // `JwtService` logs its decode reason at `debug`.
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
        // Constant-time over the contents; a length mismatch, which is not secret, is
        // unequal at once.
        if verify_slices_are_equal(tx.csrf.as_bytes(), state.as_bytes()).is_err() {
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
        // `oauth2` drops the answer's status, without which a provider's `503`
        // would read as the caller's failure.
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
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            exchange_failed(&endpoint, answered, error)
        })?;
        Ok(TokenSet {
            access_token: token.access_token().secret().clone(),
            id_token: None,
            refresh_token: token.refresh_token().map(|t| t.secret().clone()),
        })
    }

    /// Authenticated `GET` against any provider endpoint (GitHub's verified emails,
    /// say), deserialized into `T`, through this client's redirect-refusing backend.
    /// A `url` that is not `https` is refused before anything is sent: the call
    /// carries the bearer token.
    pub async fn fetch<T: DeserializeOwned>(
        &self,
        url: &str,
        access_token: &str,
    ) -> Result<T, AuthError> {
        let endpoint = Endpoint {
            role: PROVIDER_ENDPOINT,
            url,
        };
        if !reqwest::Url::parse(url.trim()).is_ok_and(|url| url.scheme() == "https") {
            return Err(AuthError::Failed(format!(
                "{endpoint} is not an https URL, and RFC 6750 §5.3 sends a bearer token over TLS \
                 alone"
            )));
        }
        self.read(&endpoint, access_token).await
    }

    /// Fetch the caller's profile from the configured `userinfo_url`, deserialized into `T`.
    pub async fn userinfo<T: DeserializeOwned>(&self, access_token: &str) -> Result<T, AuthError> {
        let endpoint = Endpoint {
            role: USERINFO_ENDPOINT,
            url: &self.config.userinfo_url,
        };
        self.read(&endpoint, access_token).await
    }

    /// Anything but a `2xx` fails, a `3xx` included: no redirect is followed.
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
            tls: ClientTls::default(),
        }
    }

    /// A client over `config` whatever it holds, built past `new`'s refusals to
    /// reach what runs after them.
    fn unchecked(config: OAuthClientConfig) -> OAuthClient {
        OAuthClient {
            config,
            http: backend().build().expect("the backend builds"),
        }
    }

    fn jwt() -> JwtService {
        JwtService::new(JwtOptions::new("oauth-client-tests-padded-to-32b")).expect("HMAC service")
    }

    fn signed_transaction(jwt: &JwtService, provider: &str, csrf: &str) -> String {
        jwt.sign_handshake(
            TRANSACTION_PURPOSE,
            &Transaction {
                typ: TransactionKind::OauthTx,
                provider: provider.into(),
                csrf: csrf.into(),
                pkce: "verifier".into(),
                exp: jwt.expiry(),
            },
        )
        .expect("sign")
    }

    #[test]
    fn new_rejects_invalid_config_at_validate_stage() {
        // `OAuthClient` is not `Debug`, hence `is_err`.
        assert!(OAuthClient::new(OAuthClientConfig::default()).is_err());
    }

    #[test]
    fn new_accepts_a_valid_config() {
        assert!(OAuthClient::new(valid_config()).is_ok());
        OAuthClient::basic_client(&valid_config()).expect("basic_client builds");
    }

    /// The URL may carry userinfo, and the sentence is logged: it names the
    /// field and the parser's fixed reason, never the value.
    #[test]
    fn basic_client_refuses_a_malformed_url_naming_its_field_and_not_its_value() {
        let mut config = valid_config();
        config.auth_url = "https://u:s3cr3t@idp:99999/x".into();
        let Err(AuthError::Failed(msg)) = OAuthClient::basic_client(&config) else {
            panic!("expected Failed");
        };
        assert_eq!(
            msg, "OAuthClientConfig::auth_url does not parse as a URL: invalid port number",
            "{msg}"
        );

        config.auth_url = "not a url".into();
        let Err(AuthError::Failed(msg)) = OAuthClient::basic_client(&config) else {
            panic!("expected Failed");
        };
        assert!(
            msg.contains("relative URL without a base") && !msg.contains("not a url"),
            "{msg}"
        );
    }

    /// A rule failing on a value `validator` would list among its parameters.
    fn always_refused(_: &str) -> Result<(), validator::ValidationError> {
        Err(validator::ValidationError::new("refused"))
    }

    #[derive(Validate)]
    struct Probe {
        #[validate(custom(function = "always_refused"))]
        client_secret: String,
    }

    /// `validator`'s `Display` prints a failed rule's parameters, the rejected
    /// value among them; the refusal says the field and the rule alone.
    #[test]
    fn a_validation_failure_says_the_field_and_the_rule_and_never_the_value() {
        let errors = Probe {
            client_secret: "s3cr3t".into(),
        }
        .validate()
        .expect_err("the rule refuses every value");
        assert!(
            errors.to_string().contains("s3cr3t"),
            "the probe quotes: {errors}"
        );

        let AuthError::Failed(sentence) = invalid(errors) else {
            panic!("a refused config is a failed authentication setup");
        };
        assert!(sentence.contains("client_secret: refused"), "{sentence}");
        assert!(!sentence.contains("s3cr3t"), "{sentence}");
        assert!(!sentence.contains("value"), "{sentence}");
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
        let mut config = valid_config();
        config.redirect_url = "/relative/path".into();
        assert!(matches!(
            OAuthClient::basic_client(&config),
            Err(AuthError::Failed(_))
        ));
    }

    #[test]
    fn authorize_surfaces_basic_client_error() {
        let mut config = valid_config();
        config.auth_url = "not a url".into();
        assert!(
            matches!(OAuthClient::new(config.clone()), Err(AuthError::Failed(_))),
            "new refuses it first"
        );
        assert!(matches!(
            unchecked(config).authorize(&jwt(), "acme"),
            Err(AuthError::Failed(_))
        ));
    }

    #[tokio::test]
    async fn exchange_rejects_a_transaction_minted_for_another_provider() {
        let jwt = jwt();
        let client = OAuthClient::new(valid_config()).expect("new accepts a valid config");
        let transaction = signed_transaction(&jwt, "provider-a", "agreed-state");

        let Err(err) = client
            .exchange(&jwt, "provider-b", &transaction, "agreed-state", "code")
            .await
        else {
            panic!("a transaction from another provider is rejected");
        };
        assert!(err.to_string().contains("provider mismatch"), "{err}");
    }

    /// The token endpoint does not parse, so a state that passes fails there instead.
    #[tokio::test]
    async fn the_state_matches_only_byte_for_byte_whatever_its_length() {
        let jwt = jwt();
        let mut config = valid_config();
        config.token_url = "::::".into();
        let client = unchecked(config);
        let transaction = signed_transaction(&jwt, "acme", "the-signed-state");

        for (presented, matches) in [
            ("the-signed-state", true),
            ("the-signed-statE", false),
            ("the-signed-stat", false),
            ("the-signed-states", false),
            ("", false),
        ] {
            let Err(err) = client
                .exchange(&jwt, "acme", &transaction, presented, "the-code")
                .await
            else {
                panic!("nothing reaches a token endpoint that does not parse");
            };
            assert!(matches!(err, AuthError::Failed(_)), "{presented:?}: {err}");
            assert_eq!(
                !err.to_string().contains("state mismatch"),
                matches,
                "{presented:?}: {err}"
            );
        }
    }

    /// `fetch` refuses a plaintext URL before `read`; the backend refuses it
    /// too, for every path that reaches `read` past the validators.
    #[tokio::test]
    async fn the_backend_dials_no_plaintext_url_whatever_path_reaches_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let addr = listener.local_addr().expect("its address");
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok(_dropped) = listener.accept().await {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let url = format!("http://{addr}/user/emails");

        let Err(AuthError::Unavailable { detail, .. }) = OAuthClient::new(valid_config())
            .expect("new accepts a valid config")
            .read::<serde_json::Value>(
                &Endpoint {
                    role: PROVIDER_ENDPOINT,
                    url: &url,
                },
                "never-sent",
            )
            .await
        else {
            panic!("a plaintext URL is no call the backend makes");
        };

        tokio::task::yield_now().await;
        assert_eq!(
            accepted.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing was dialled: {detail}"
        );
        assert!(
            detail.starts_with(&format!(
                "the OAuth provider's endpoint (http://{addr}/user/emails) could not be called: "
            )),
            "{detail}"
        );
    }

    /// A name lookup that never answers.
    struct NeverResolves;

    impl reqwest::dns::Resolve for NeverResolves {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(std::future::pending())
        }
    }

    /// A provider nobody can reach fails at `CONNECT_TIMEOUT`, naming that bound. A
    /// resolver stalls it: no local listener can leave a handshake hanging.
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
