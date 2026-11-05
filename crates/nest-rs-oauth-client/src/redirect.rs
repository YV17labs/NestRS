//! [`AuthorizationRedirect`] — the answer to an authorization request
//! (RFC 6749 §4.1.1).

use nest_rs_authn::SignError;
use nest_rs_http::__private::{AnswerDoc, DocumentedAnswer};
use nest_rs_http::{HeaderValue, IntoResponse, Response, StatusCode, header};
use oauth2::url::Url;

use crate::client::OAuthClient;

/// The redirect leg of the flow, produced by [`OAuthClient::authorize`]: the
/// answer to the authorization request (RFC 6749 §4.1.1). A handler returns
/// it, bare or inside a `Result`, and it answers a `302` sending the user agent
/// to the provider's authorization endpoint, which sets the flow's transaction
/// cookie — `__Host-oauth_tx=<token>; Path=/; Max-Age=600; HttpOnly; Secure;
/// SameSite=Lax` — and `Cache-Control: no-store`, so no cache keeps the response
/// that set it; the route documents its `302` and `Location` by the type.
///
/// `SameSite=Lax`, not `Strict`: the callback is a top-level cross-site
/// navigation from the provider, which `Strict` would strip (RFC 6265bis
/// §5.6.7.1). `Max-Age` is the signed token's own lifetime,
/// [`OAuthClient::TRANSACTION_TTL_SECS`].
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_authn::JwtService;
/// use nest_rs_http::{Result, controller, routes};
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
///         Ok(self.client.authorize(&self.jwt, "acme", None)?)
///     }
/// }
/// ```
///
/// Its location and its cookie are written once, valid, and read by nothing
/// but the answer, so the cookie leaves with `no-store` alone and what no
/// header can carry never reaches a handler.
///
/// ```compile_fail,E0616
/// # fn read(redirect: nest_rs_oauth_client::AuthorizationRedirect) {
/// let _cookie = redirect.set_cookie;
/// # }
/// ```
pub struct AuthorizationRedirect {
    location: HeaderValue,
    set_cookie: HeaderValue,
}

impl AuthorizationRedirect {
    /// The redirect to `url`, setting `transaction` as the flow's cookie;
    /// refused, quoting neither, when one is no header value.
    pub(crate) fn new(url: &Url, transaction: &str) -> Result<Self, SignError> {
        let location = HeaderValue::from_str(url.as_str()).map_err(|error| {
            SignError::because("the provider's authorization URL is no header value", error)
        })?;
        let set_cookie = HeaderValue::try_from(format!(
            "{}={transaction}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Lax",
            OAuthClient::TRANSACTION_COOKIE,
            OAuthClient::TRANSACTION_TTL_SECS,
        ))
        .map_err(|error| SignError::because("the transaction cookie is no header value", error))?;
        Ok(Self {
            location,
            set_cookie,
        })
    }

    /// The provider authorization URL the user agent is redirected to.
    #[expect(
        clippy::expect_used,
        reason = "`new` read it off a `Url`, which serializes to ASCII alone"
    )]
    pub fn url(&self) -> &str {
        self.location.to_str().expect("a URL is ASCII")
    }
}

impl IntoResponse for AuthorizationRedirect {
    fn into_response(self) -> Response {
        Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, self.location)
            .header(header::SET_COOKIE, self.set_cookie)
            .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .finish()
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
    use super::*;

    fn provider() -> Url {
        Url::parse("https://provider.example/authorize?state=s").expect("a URL")
    }

    #[test]
    fn the_transaction_cookie_is_host_prefixed_secure_and_lives_as_long_as_its_token() {
        let redirect = AuthorizationRedirect::new(&provider(), "tok").expect("valid");
        let cookie = redirect.set_cookie.to_str().expect("ASCII");
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

    #[test]
    fn the_url_it_redirects_to_reads_back_as_written() {
        let redirect = AuthorizationRedirect::new(&provider(), "tok").expect("valid");
        assert_eq!(redirect.url(), provider().as_str());
    }

    /// What a header cannot carry never becomes an answer, and its refusal
    /// quotes neither field.
    #[test]
    fn what_no_header_can_carry_is_refused_quoting_it_never() {
        let Err(refused) = AuthorizationRedirect::new(&provider(), "never\nquoted") else {
            panic!("a cookie no header can carry was written");
        };
        let said = nest_rs_core::error_message(&refused);
        assert!(said.contains("the transaction cookie"), "{said}");
        assert!(!said.contains("never"), "{said}");
    }
}
