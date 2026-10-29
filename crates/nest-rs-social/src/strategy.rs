//! [`SocialStrategy`] — RFC 6749 §4.1.2's authorization response at a social
//! provider's redirect URI, read into the app's principal through the
//! product's [`SocialSignIn`].

use std::sync::Arc;

use nest_rs_authn::{AuthError, JwtService, PrincipalIdentity, Strategy};
use nest_rs_core::{AmbientValue, injectable};
use nest_rs_http::{Body, FromRequest, Query, Request, RequestBody, async_trait, header};
use nest_rs_oauth_client::OAuthClient;
use serde::Deserialize;

use crate::provider::SocialProfile;
use crate::registry::SocialRegistry;

/// What the product decides at the end of a social sign-in: who the
/// provider's [`SocialProfile`] is in this app. The flow before it is
/// [`SocialStrategy`]'s.
///
/// ```
/// use nest_rs_authn::{AuthError, PrincipalIdentity};
/// use nest_rs_core::{AmbientValue, injectable};
/// use nest_rs_http::async_trait;
/// use nest_rs_social::{SocialProfile, SocialSignIn};
///
/// #[derive(Clone)]
/// struct Member {
///     id: String,
///     resource: Option<String>,
/// }
///
/// impl AmbientValue for Member {}
///
/// impl PrincipalIdentity for Member {
///     fn actor_id(&self) -> Option<String> {
///         Some(self.id.clone())
///     }
/// }
///
/// #[injectable]
/// #[derive(Default)]
/// struct MembersSignIn;
///
/// #[async_trait]
/// impl SocialSignIn for MembersSignIn {
///     type Principal = Member;
///
///     async fn sign_in(
///         &self,
///         profile: SocialProfile,
///         resource: Option<&str>,
///     ) -> Result<Member, AuthError> {
///         Ok(Member {
///             id: format!("{}:{}", profile.provider, profile.subject),
///             resource: resource.map(str::to_owned),
///         })
///     }
/// }
/// ```
#[async_trait]
pub trait SocialSignIn: Send + Sync + 'static {
    /// The principal a sign-in resolves to — what the callback route's
    /// handler reads as `Ctx<Principal>`.
    type Principal: PrincipalIdentity + AmbientValue + Clone;

    /// The principal `profile` signs in as, keyed on its
    /// [`subject`](SocialProfile::subject), never its email. `resource` is the
    /// one (RFC 8707) the flow's own access token was asked for when it began
    /// ([`SocialProvider::authorize`](crate::SocialProvider::authorize)),
    /// carried across the provider in the signed transaction; `None` when it
    /// named none.
    async fn sign_in(
        &self,
        profile: SocialProfile,
        resource: Option<&str>,
    ) -> Result<Self::Principal, AuthError>;
}

/// The [`Strategy`] of a social provider's redirect URI: a route whose path
/// names the provider `{provider}` (`/social/{provider}/callback`), bound with
/// `AuthnGuard<SocialStrategy<S>>`. It reads RFC 6749 §4.1.2's `code` and
/// `state` against the transaction cookie that began the flow
/// ([`OAuthClient::callback`]), trades the code for the provider's tokens and
/// profile, and hands the profile to the product's [`SocialSignIn`].
///
/// A provider's own refusal (§4.1.2.1's `error`) is refused as what it says:
/// `server_error` and `temporarily_unavailable` as
/// [`AuthError::Unavailable`], every other code as [`AuthError::Failed`],
/// naming the code when §4.1.2.1 registers it and never what the request
/// sent otherwise. A provider no imported module registers, a response with
/// no `code` or `state`, a missing, forged or expired transaction, or one
/// minted for another provider, is refused before any exchange.
#[injectable]
pub struct SocialStrategy<S: SocialSignIn> {
    #[inject]
    providers: Arc<SocialRegistry>,
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    sign_in: Arc<S>,
}

impl<S: SocialSignIn> SocialStrategy<S> {
    /// Construct over already-resolved parts (container or tests).
    pub fn new(providers: Arc<SocialRegistry>, jwt: Arc<JwtService>, sign_in: Arc<S>) -> Self {
        Self {
            providers,
            jwt,
            sign_in,
        }
    }
}

/// RFC 6749 §4.1.2's response parameters, and §4.1.2.1's `error`.
#[derive(Deserialize)]
struct AuthorizationParameters {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The refusal a §4.1.2.1 `error` code is answered with, its detail naming
/// the code when the registry holds it.
fn refused_by_provider(code: &str) -> AuthError {
    let detail = match code {
        "server_error" => {
            return AuthError::unavailable(
                "social provider could not authorize: server_error",
                None,
            );
        }
        "temporarily_unavailable" => {
            return AuthError::unavailable(
                "social provider could not authorize: temporarily_unavailable",
                None,
            );
        }
        "access_denied" => "social provider refused the authorization: access_denied",
        "invalid_request" => "social provider refused the authorization: invalid_request",
        "unauthorized_client" => "social provider refused the authorization: unauthorized_client",
        "unsupported_response_type" => {
            "social provider refused the authorization: unsupported_response_type"
        }
        "invalid_scope" => "social provider refused the authorization: invalid_scope",
        _ => "social provider refused the authorization with an unregistered error code",
    };
    AuthError::failed(detail)
}

#[async_trait]
impl<S: SocialSignIn> Strategy for SocialStrategy<S> {
    type Principal = S::Principal;

    async fn authenticate(&self, req: &mut Request) -> Result<S::Principal, AuthError> {
        let provider = req
            .path_param("provider")
            .ok_or_else(|| AuthError::failed("social callback route names no {provider}"))
            .map(|key| self.providers.get(key))?
            .ok_or_else(|| AuthError::failed("social callback names no registered provider"))?;
        let Ok(Query(parameters)) = Query::<AuthorizationParameters>::from_request(
            req,
            &mut RequestBody::new(Body::empty()),
        )
        .await
        else {
            return Err(AuthError::failed("social callback query does not decode"));
        };
        if let Some(code) = parameters.error {
            return Err(refused_by_provider(&code));
        }
        let code = parameters
            .code
            .filter(|code| !code.is_empty())
            .ok_or_else(|| AuthError::failed("social callback missing code"))?;
        let state = parameters
            .state
            .ok_or_else(|| AuthError::failed("social callback missing state"))?;
        let transaction = OAuthClient::transaction_from(
            req.headers()
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|field| field.to_str().ok()),
        )
        .ok_or_else(|| AuthError::failed("social transaction cookie missing"))?;

        let response =
            OAuthClient::callback(&self.jwt, provider.key(), transaction, &state, &code)?;
        let resource = response.resource().map(str::to_owned);
        let tokens = provider.exchange(response).await?;
        let profile = provider.profile(&tokens).await?;
        self.sign_in.sign_in(profile, resource.as_deref()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_that_cannot_answer_now_is_unavailable_and_any_other_refusal_failed() {
        for code in ["server_error", "temporarily_unavailable"] {
            assert!(
                matches!(refused_by_provider(code), AuthError::Unavailable { .. }),
                "{code}"
            );
        }
        for code in [
            "access_denied",
            "invalid_request",
            "unauthorized_client",
            "unsupported_response_type",
            "invalid_scope",
        ] {
            let refusal = refused_by_provider(code);
            assert!(matches!(refusal, AuthError::Failed { .. }), "{code}");
            let said = nest_rs_core::error_message(&refusal);
            assert!(said.ends_with(code), "{said}");
        }
    }

    #[test]
    fn an_unregistered_code_is_refused_without_quoting_it() {
        let refusal = refused_by_provider("<script>hunter2</script>");
        assert!(matches!(refusal, AuthError::Failed { .. }));
        let said = nest_rs_core::error_message(&refusal);
        assert!(!said.contains("hunter2"), "{said}");
        assert!(said.contains("unregistered error code"), "{said}");
    }
}
