//! [`SocialRedirect`] — the answer to a social sign-in's authorization
//! request (RFC 6749 §4.1.1).

use nest_rs_http::__private::{AnswerDoc, DocumentedAnswer};
use nest_rs_http::{IntoResponse, ProblemDetails, Response, StatusCode};
use nest_rs_oauth_client::AuthorizationRedirect;

/// The answer to a social sign-in's first leg, the authorization request
/// (RFC 6749 §4.1.1), as [`SocialRegistry::authorize`](crate::SocialRegistry::authorize)
/// builds it: the provider's [`AuthorizationRedirect`], whose `302` carries the
/// flow's transaction cookie and `Cache-Control: no-store` — or, when no
/// imported module registers a provider by that key, the `404` problem a route
/// that does not exist answers. A handler returns it bare, or inside a
/// `Result`; the route documents its `302` and `Location` by the type.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_authn::JwtService;
/// use nest_rs_http::{Path, Result, controller, routes};
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
///         Ok(self.providers.authorize(&provider, &self.jwt, None)?)
///     }
/// }
/// ```
pub struct SocialRedirect(Option<AuthorizationRedirect>);

impl SocialRedirect {
    /// The answer for `redirect`, a provider's.
    pub(crate) fn to(redirect: AuthorizationRedirect) -> Self {
        Self(Some(redirect))
    }

    /// The answer for a key no imported module registers.
    pub(crate) fn unknown() -> Self {
        Self(None)
    }
}

impl IntoResponse for SocialRedirect {
    fn into_response(self) -> Response {
        match self.0 {
            Some(redirect) => redirect.into_response(),
            None => ProblemDetails::from_status(StatusCode::NOT_FOUND).into_response(),
        }
    }
}

/// A route returning it, bare or inside a `Result`, documents its provider's
/// redirect.
impl DocumentedAnswer for SocialRedirect {
    const ANSWER: AnswerDoc = AuthorizationRedirect::ANSWER;
}

#[cfg(test)]
mod tests {
    use nest_rs_authn::{JwtOptions, JwtService};
    use nest_rs_oauth_client::{OAuthClient, OAuthClientConfig};

    use super::*;

    fn begun() -> AuthorizationRedirect {
        let client = OAuthClient::new(OAuthClientConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://provider.example/authorize".into(),
            token_url: "https://provider.example/token".into(),
            redirect_url: "https://app.example/callback".into(),
            userinfo_url: "https://provider.example/userinfo".into(),
            scopes: vec!["read".into()],
            authorization_parameters: Default::default(),
            tls: nest_rs_config::ClientTls::default(),
        })
        .expect("a valid client");
        let mut options = JwtOptions::new("test-secret-padded-to-thirty-two-b");
        options.issuer = Some("https://app.example".into());
        options.audience = Some("https://app.example".into());
        let jwt = JwtService::new(options).expect("an HMAC JwtService");
        client
            .authorize(&jwt, "provider", None)
            .expect("the transaction signs")
    }

    /// Each flow draws its own `state` and transaction, so what two answers
    /// share is everything but those: the status and the headers, in order,
    /// with the cookie's attributes and the location's endpoint.
    #[test]
    fn a_social_redirect_answers_what_the_plain_client_answers() {
        let shape = |response: &Response| {
            response
                .headers()
                .iter()
                .map(|(name, value)| {
                    let value = value.to_str().expect("ASCII");
                    let fixed = match name.as_str() {
                        "location" => value
                            .split_once('?')
                            .map_or(value, |(endpoint, _)| endpoint),
                        "set-cookie" => value.split_once(';').map_or(value, |(_, rest)| rest),
                        _ => value,
                    };
                    (name.clone(), fixed.to_owned())
                })
                .collect::<Vec<_>>()
        };

        let plain = begun().into_response();
        let social = SocialRedirect::to(begun()).into_response();

        assert_eq!(social.status(), plain.status());
        assert_eq!(shape(&social), shape(&plain));
    }
}
