//! The social-login provider contract.

use std::future::Future;
use std::pin::Pin;

use nest_rs_authn::{AuthError, JwtService};
use nest_rs_core::Secret;
use nest_rs_oauth_client::{AuthorizationRedirect, AuthorizationResponse, OAuthClient, TokenSet};

/// The normalized profile a provider reports for the authenticated caller.
///
/// Identity resolution keys on [`subject`](SocialProfile::subject), never the
/// email. The email and the name are personal data, so each is a [`Secret`]:
/// `Debug` prints only whether it is there.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct SocialProfile {
    /// Provider key, e.g. `"github"` — must match [`SocialProvider::key`].
    pub provider: &'static str,
    /// Provider-side stable identifier (GitHub numeric id, OIDC `sub`).
    pub subject: String,
    /// The account email, if the provider returned one.
    pub email: Option<Secret<String>>,
    /// Whether the provider attests the email. Drives the linking rule: only a
    /// verified email may match an existing account.
    pub email_verified: bool,
    /// The account display name, if the provider returned one.
    pub name: Option<Secret<String>>,
}

impl SocialProfile {
    /// A profile with no email and `email_verified = false`. Chain the
    /// `with_*` setters to fill the optional fields.
    pub fn new(provider: &'static str, subject: impl Into<String>) -> Self {
        Self {
            provider,
            subject: subject.into(),
            email: None,
            email_verified: false,
            name: None,
        }
    }

    /// Set the email and whether the provider attests it in one call — the two
    /// travel together so a caller cannot mark an absent email verified.
    pub fn with_email(mut self, email: Option<String>, verified: bool) -> Self {
        self.email = email.filter(|e| !e.is_empty()).map(Secret::new);
        self.email_verified = verified && self.email.is_some();
        self
    }

    /// Set the display name, treating an empty string as absent.
    pub fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name.filter(|n| !n.is_empty()).map(Secret::new);
        self
    }
}

/// The boxed future returned by [`SocialProvider::exchange`].
pub type TokenFuture<'a> = Pin<Box<dyn Future<Output = Result<TokenSet, AuthError>> + Send + 'a>>;
/// The boxed future returned by [`SocialProvider::profile`].
pub type ProfileFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SocialProfile, AuthError>> + Send + 'a>>;

/// The behavioral contract a social login provider implements — the open seam
/// third-party provider crates extend. Standard OIDC/OAuth2 providers override
/// only [`profile`](Self::profile); the default `authorize`/`exchange` drive the
/// shared PKCE/CSRF flow.
pub trait SocialProvider: Send + Sync + 'static {
    /// Stable route/config key: lowercase, no spaces (`"github"`). Matches the
    /// registry entry key and [`SocialProfile::provider`].
    fn key(&self) -> &'static str;

    /// The configured base-flow client. The default `authorize`/`exchange`
    /// drive the shared PKCE/CSRF flow through it.
    fn client(&self) -> &OAuthClient;

    /// Begin the redirect leg (default: the shared flow); the transaction is
    /// bound to [`key`](Self::key), so it cannot complete on another provider's
    /// callback, and carries `resource` (RFC 8707), the one this app's own
    /// token is asked for, which the callback reads back
    /// ([`AuthorizationResponse::resource`]).
    fn authorize(
        &self,
        jwt: &JwtService,
        resource: Option<&str>,
    ) -> Result<AuthorizationRedirect, AuthError> {
        self.client().authorize(jwt, self.key(), resource)
    }

    /// Trade the callback's authorization response — already matched to the
    /// transaction that began the flow ([`OAuthClient::callback`]) — for the
    /// provider's tokens. Default: the shared flow (PKCE, SSRF-safe client).
    /// Override **only** for a provider whose exchange deviates (e.g. Apple's
    /// per-request ES256-signed client secret, or an OIDC provider populating
    /// [`TokenSet::id_token`]), building its [`TokenSet`] with
    /// [`TokenSet::new`].
    fn exchange(&self, response: AuthorizationResponse) -> TokenFuture<'_> {
        Box::pin(async move { self.client().exchange(response).await })
    }

    /// Fetch and normalize the provider's profile. Takes the full [`TokenSet`]
    /// so an OIDC provider may read identity from the id_token rather than a
    /// userinfo endpoint. No default — this is the per-provider code.
    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_prints_whether_the_personal_data_is_there_and_never_what() {
        let profile = SocialProfile::new("github", "42")
            .with_email(Some("ada@example.com".into()), true)
            .with_name(Some("Ada Lovelace".into()));
        let printed = format!("{profile:?}");
        assert!(!printed.contains("ada@example.com"), "{printed}");
        assert!(!printed.contains("Lovelace"), "{printed}");
        assert!(printed.contains("email: Some(<redacted>)"), "{printed}");
        assert!(printed.contains("42"), "the subject stays: {printed}");
        assert_eq!(
            profile
                .email
                .as_ref()
                .map(Secret::expose_secret)
                .map(String::as_str),
            Some("ada@example.com")
        );
    }
}
