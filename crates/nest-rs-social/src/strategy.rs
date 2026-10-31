//! [`SocialStrategy`] — RFC 6749 §4.1.2's authorization response at a social
//! provider's redirect URI, read into the app's principal through the
//! product's [`SocialSignIn`].

use std::sync::Arc;

use nest_rs_authn::{AuthError, JwtService, PrincipalIdentity, Strategy};
use nest_rs_core::{AmbientValue, async_trait, injectable};
use nest_rs_http::{Request, header};
use nest_rs_oauth_client::OAuthClient;

use crate::provider::SocialProfile;
use crate::registry::SocialRegistry;

/// What the product decides at the end of a social sign-in: who the
/// provider's [`SocialProfile`] is in this app. The flow before it is
/// [`SocialStrategy`]'s.
///
/// ```
/// use nest_rs_authn::{AuthError, PrincipalIdentity};
/// use nest_rs_core::{async_trait, injectable};
/// use nest_rs_social::{SocialProfile, SocialSignIn};
///
/// #[derive(Clone, PrincipalIdentity)]
/// struct Member {
///     #[principal(actor)]
///     id: String,
///     resource: Option<String>,
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
/// `AuthnGuard<SocialStrategy<S>>`. It reads RFC 6749 §4.1.2's response
/// against the transaction cookie that began the flow
/// ([`OAuthClient::callback`]), trades the code for the provider's tokens and
/// profile, and hands the profile to the product's [`SocialSignIn`].
///
/// A provider's own refusal (§4.1.2.1's `error`) is read only once the
/// response matches its transaction, and is refused as what it says:
/// `server_error` and `temporarily_unavailable` as
/// [`AuthError::Unavailable`], every other code as [`AuthError::Failed`]. A
/// provider no imported module registers, a missing, forged or expired
/// transaction, one minted for another provider or carrying another `state`,
/// and a response with no `code`, are refused before any exchange.
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

#[async_trait]
impl<S: SocialSignIn> Strategy for SocialStrategy<S> {
    type Principal = S::Principal;

    async fn authenticate(&self, req: &mut Request) -> Result<S::Principal, AuthError> {
        let key = req
            .path_param("provider")
            .ok_or_else(|| AuthError::failed("social callback route names no {provider}"))?;
        let provider = self
            .providers
            .get(key)
            .ok_or_else(|| AuthError::failed("social callback names no registered provider"))?;
        let transaction = OAuthClient::transaction_from(
            req.headers()
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|field| field.to_str().ok()),
        )
        .ok_or_else(|| AuthError::failed("social transaction cookie missing"))?;

        let response = OAuthClient::callback(
            &self.jwt,
            provider.key(),
            transaction,
            req.uri().query().unwrap_or_default(),
        )?;
        let resource = response.resource().map(str::to_owned);
        let tokens = provider.exchange(response).await?;
        let profile = provider.profile(&tokens).await?;
        self.sign_in.sign_in(profile, resource.as_deref()).await
    }
}
