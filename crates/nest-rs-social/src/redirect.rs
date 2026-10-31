//! [`SocialRedirect`] — the answer to a social sign-in's authorization
//! request (RFC 6749 §4.1.1).

use nest_rs_authn::AuthError;
use nest_rs_http::__private::{AnswerDoc, DocumentedAnswer};
use nest_rs_http::{HeaderValue, IntoResponse, ProblemDetails, Response, StatusCode, header};
use nest_rs_oauth_client::AuthorizationRedirect;

/// The answer to a social sign-in's first leg, the authorization request
/// (RFC 6749 §4.1.1), as [`SocialRegistry::authorize`](crate::SocialRegistry::authorize)
/// builds it: a `302` sending the user agent to the provider's authorization
/// endpoint, which sets the flow's transaction cookie
/// ([`AuthorizationRedirect::set_cookie`]) and `Cache-Control: no-store`, so
/// no cache keeps the response that set it — or, when no imported module
/// registers a provider by that key, the `404` problem a route that does not
/// exist answers. A handler returns it bare, or inside a `Result`; the route
/// documents its `302` and `Location` by the type.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_authn::JwtService;
/// use nest_rs_http::{Opaque, Path, Result, controller, routes};
/// use nest_rs_social::{SocialRedirect, SocialRegistry};
///
/// #[controller(path = "/social")]
/// struct SignInController {
///     #[inject]
///     providers: Arc<SocialRegistry>,
///     #[inject]
///     jwt: Arc<JwtService>,
/// }
///
/// #[routes]
/// impl SignInController {
///     #[get("/{provider}/authorize")]
///     #[public]
///     async fn authorize(&self, Path(provider): Path<String>) -> Result<SocialRedirect> {
///         self.providers.authorize(&provider, &self.jwt, None).opaque()
///     }
/// }
/// ```
pub struct SocialRedirect(Destination);

/// Where a [`SocialRedirect`] sends the user agent.
enum Destination {
    /// The provider's authorization endpoint, and the transaction cookie.
    Provider {
        location: HeaderValue,
        transaction: HeaderValue,
    },
    /// No imported module registers a provider by the key asked for.
    Unknown,
}

impl SocialRedirect {
    /// The answer for `redirect`, a provider's: refused when its URL or its
    /// cookie is no header value.
    pub(crate) fn to(redirect: &AuthorizationRedirect) -> Result<Self, AuthError> {
        let location = HeaderValue::try_from(redirect.url.as_str()).map_err(|error| {
            AuthError::failed_because("the provider's authorization URL is no header value", error)
        })?;
        let transaction = HeaderValue::try_from(redirect.set_cookie()).map_err(|error| {
            AuthError::failed_because("the transaction cookie is no header value", error)
        })?;
        Ok(Self(Destination::Provider {
            location,
            transaction,
        }))
    }

    /// The answer for a key no imported module registers.
    pub(crate) fn unknown() -> Self {
        Self(Destination::Unknown)
    }
}

impl IntoResponse for SocialRedirect {
    fn into_response(self) -> Response {
        let Destination::Provider {
            location,
            transaction,
        } = self.0
        else {
            return ProblemDetails::from_status(StatusCode::NOT_FOUND).into_response();
        };
        let mut response = Response::builder().status(StatusCode::FOUND).finish();
        let headers = response.headers_mut();
        headers.insert(header::LOCATION, location);
        headers.insert(header::SET_COOKIE, transaction);
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

/// A route returning it, bare or inside a `Result`, documents its `302` and
/// its `Location`.
impl DocumentedAnswer for SocialRedirect {
    const ANSWER: AnswerDoc = AnswerDoc {
        status: 302,
        location: true,
        next_link: false,
    };
}
