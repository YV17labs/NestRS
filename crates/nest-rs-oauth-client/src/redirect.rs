//! [`AuthorizationRedirect`] — the answer to an authorization request
//! (RFC 6749 §4.1.1).

use nest_rs_authn::AuthError;
use nest_rs_core::error_message;
use nest_rs_http::__private::{AnswerDoc, DocumentedAnswer};
use nest_rs_http::{HeaderValue, IntoResponse, ProblemDetails, Response, StatusCode, header};

use crate::client::OAuthClient;

/// The redirect leg of the flow, produced by [`OAuthClient::authorize`]: the
/// answer to the authorization request (RFC 6749 §4.1.1). A handler returns
/// it, bare or inside a `Result`, and it answers a `302` sending the user agent
/// to the provider's authorization endpoint, which sets the flow's transaction
/// cookie ([`set_cookie`](Self::set_cookie)) and `Cache-Control: no-store`, so
/// no cache keeps the response that set it; the route documents its `302` and
/// `Location` by the type.
///
/// What no header can carry, a URL a provider's override appended to, answers
/// the opaque `500` and files what failed at `error`, quoting neither field.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_authn::JwtService;
/// use nest_rs_http::{Opaque, Result, controller, routes};
/// use nest_rs_oauth_client::{AuthorizationRedirect, OAuthClient};
///
/// #[controller(path = "/")]
/// struct SignInController {
///     #[inject]
///     client: Arc<OAuthClient>,
///     #[inject]
///     jwt: Arc<JwtService>,
/// }
///
/// #[routes]
/// impl SignInController {
///     #[get("/authorize")]
///     #[public]
///     async fn authorize(&self) -> Result<AuthorizationRedirect> {
///         self.client.authorize(&self.jwt, "acme", None).opaque()
///     }
/// }
/// ```
#[non_exhaustive]
pub struct AuthorizationRedirect {
    /// The provider authorization URL to redirect the user agent to.
    pub url: String,
    /// Signed, short-lived token binding the CSRF state to the PKCE verifier.
    /// Set as a cookie on the redirect through [`set_cookie`](Self::set_cookie);
    /// read back with [`OAuthClient::transaction_from`] and passed to
    /// [`callback`](OAuthClient::callback).
    pub transaction: String,
}

impl AuthorizationRedirect {
    /// The `Set-Cookie` value carrying [`transaction`](Self::transaction) on the
    /// redirect: `__Host-oauth_tx=<token>; Path=/; Max-Age=600; HttpOnly; Secure;
    /// SameSite=Lax`.
    ///
    /// `SameSite=Lax`, not `Strict`: the callback is a top-level cross-site
    /// navigation from the provider, which `Strict` would strip (RFC 6265bis
    /// §5.6.7.1). `Max-Age` is the signed token's own lifetime,
    /// [`OAuthClient::TRANSACTION_TTL_SECS`].
    pub fn set_cookie(&self) -> String {
        format!(
            "{}={}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Lax",
            OAuthClient::TRANSACTION_COOKIE,
            self.transaction,
            OAuthClient::TRANSACTION_TTL_SECS,
        )
    }

    /// The `302`, refused when its URL or its cookie is no header value.
    fn answer(self) -> Result<Response, AuthError> {
        let set_cookie = self.set_cookie();
        let location = HeaderValue::try_from(self.url).map_err(|error| {
            AuthError::failed_because("the provider's authorization URL is no header value", error)
        })?;
        let transaction = HeaderValue::try_from(set_cookie).map_err(|error| {
            AuthError::failed_because("the transaction cookie is no header value", error)
        })?;
        Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location)
            .header(header::SET_COOKIE, transaction)
            .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .finish())
    }
}

impl IntoResponse for AuthorizationRedirect {
    fn into_response(self) -> Response {
        self.answer().unwrap_or_else(|error| {
            tracing::error!(
                target: crate::TARGET,
                error = %error_message(&error),
                "OAuth redirect rejected",
            );
            ProblemDetails::internal().into_response()
        })
    }
}

/// A route returning it, bare or inside a `Result`, documents its `302` and
/// its `Location`.
impl DocumentedAnswer for AuthorizationRedirect {
    const ANSWER: AnswerDoc = AnswerDoc {
        status: 302,
        location: true,
        next_link: false,
    };
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::TARGET;

    fn redirect() -> AuthorizationRedirect {
        AuthorizationRedirect {
            url: "https://provider.example/authorize".into(),
            transaction: "tok".into(),
        }
    }

    #[test]
    fn the_transaction_cookie_is_host_prefixed_secure_and_lives_as_long_as_its_token() {
        let cookie = redirect().set_cookie();
        assert!(
            cookie.starts_with(&format!("{}=tok;", OAuthClient::TRANSACTION_COOKIE)),
            "{cookie}"
        );
        let attributes: Vec<&str> = cookie.split(';').skip(1).map(str::trim).collect();
        for expected in [
            "Path=/",
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
            &format!("Max-Age={}", OAuthClient::TRANSACTION_TTL_SECS),
        ] {
            assert!(attributes.contains(&expected), "{expected}: {cookie}");
        }
        assert!(
            !cookie.to_ascii_lowercase().contains("domain"),
            "a __Host- cookie carries no Domain: {cookie}"
        );
        assert_eq!(OAuthClient::TRANSACTION_COOKIE, "__Host-oauth_tx");
        assert_eq!(OAuthClient::TRANSACTION_TTL_SECS, 600);
    }

    /// A provider overriding `authorize` may append to the URL it was handed,
    /// so what a header cannot carry is refused where the answer is written.
    #[tokio::test]
    async fn what_no_header_can_carry_answers_the_opaque_500_and_files_what_failed_quoting_neither()
    {
        let refused = [
            (
                AuthorizationRedirect {
                    url: "https://provider.example/authorize?state=never\nquoted".into(),
                    ..redirect()
                },
                "the provider's authorization URL is no header value",
            ),
            (
                AuthorizationRedirect {
                    transaction: "never\nquoted".into(),
                    ..redirect()
                },
                "the transaction cookie is no header value",
            ),
        ];
        for (redirect, what_failed) in refused {
            let logs = LogCapture::install();

            let response = redirect.into_response();

            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            for sent in [header::LOCATION, header::SET_COOKIE, header::CACHE_CONTROL] {
                assert!(response.headers().get(&sent).is_none(), "{sent}");
            }
            let document = response.into_body().into_string().await.expect("a body");
            let opaque = ProblemDetails::internal()
                .into_response()
                .into_body()
                .into_string()
                .await
                .expect("a body");
            assert_eq!(document, opaque);
            let event = logs.expect_one(TARGET, "OAuth redirect rejected");
            assert_eq!(event.level, "error");
            let error = event.field("error").expect("the chain");
            assert!(error.contains(what_failed), "{error}");
            assert!(!error.contains("never"), "{error}");
        }
    }
}
