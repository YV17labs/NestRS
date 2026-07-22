//! The social-login provider contract.
//!
//! `SocialProvider` is **flow-owning**: `authorize` and `exchange` carry
//! default implementations that drive the shared PKCE/CSRF Authorization-Code
//! flow through the provider's [`OAuth2Client`]. A provider writes nothing for
//! the common case — GitHub and Google override only [`profile`], the one
//! method whose per-provider code justifies the crate. A provider whose
//! protocol genuinely deviates (e.g. Apple's per-request ES256-signed client
//! secret, or reading identity from an id_token instead of a userinfo
//! endpoint) overrides `exchange` too — **without changing this trait**, so
//! the third-party ecosystem never breaks on a new provider shape.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use nest_rs_authn::{AuthError, Authorization, JwtService, OAuth2Client, TokenSet};

/// The normalized profile a provider reports for the authenticated caller.
///
/// `#[non_exhaustive]` + [`new`](SocialProfile::new): third-party crates build
/// it through the constructor and field setters, so adding a field later
/// (avatar, locale) is not a breaking change. Identity resolution keys on
/// [`subject`](SocialProfile::subject) — never the email (see the demo's
/// `UsersService::resolve_social_identity`).
///
/// [`Debug`] is hand-written to **redact** the PII fields (`email`, `name`):
/// it prints their presence (`Some`/`None`) but never their value, so a
/// `tracing` call that captures a profile cannot leak a user's email into logs
/// — the same fail-closed posture the config secrets and response masking take.
#[non_exhaustive]
#[derive(Clone)]
pub struct SocialProfile {
    /// Provider key, e.g. `"github"` — must match [`SocialProvider::key`].
    pub provider: &'static str,
    /// Provider-side stable identifier (GitHub numeric id, OIDC `sub`).
    pub subject: String,
    /// The account email, if the provider returned one.
    pub email: Option<String>,
    /// Whether the provider attests the email. Drives the linking rule: only a
    /// verified email may match an existing account.
    pub email_verified: bool,
    /// The account display name, if the provider returned one.
    pub name: Option<String>,
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
        self.email = email.filter(|e| !e.is_empty());
        self.email_verified = verified && self.email.is_some();
        self
    }

    /// Set the display name, treating an empty string as absent.
    pub fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name.filter(|n| !n.is_empty());
        self
    }
}

impl fmt::Debug for SocialProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Presence, never value: `Some(<redacted>)` / `None` keeps the shape
        // useful for debugging while the PII stays out of the log line.
        let redact = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("SocialProfile")
            .field("provider", &self.provider)
            .field("subject", &self.subject)
            .field("email", &redact(&self.email))
            .field("email_verified", &self.email_verified)
            .field("name", &redact(&self.name))
            .finish()
    }
}

/// Boxed futures keep the trait object-safe without an `async-trait`
/// dependency — mirrors `nest-rs-health`'s `IndicatorFuture`.
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
    fn client(&self) -> &OAuth2Client;

    /// Begin the redirect leg. Default: the shared flow. Overriding this is
    /// almost never needed.
    /// The transaction is bound to [`key`](Self::key), so a flow started here
    /// cannot be completed on another provider's callback.
    fn authorize(&self, jwt: &JwtService) -> Result<Authorization, AuthError> {
        self.client().authorize(jwt, self.key())
    }

    /// Complete the code exchange. Default: the shared flow (CSRF check, PKCE,
    /// SSRF-safe client). Override **only** for a provider whose exchange
    /// deviates (e.g. Apple's per-request ES256-signed client secret, or an
    /// OIDC provider populating [`TokenSet::id_token`]).
    fn exchange<'a>(
        &'a self,
        jwt: &'a JwtService,
        transaction: &'a str,
        state: &'a str,
        code: &'a str,
    ) -> TokenFuture<'a> {
        Box::pin(async move {
            self.client()
                .exchange(jwt, self.key(), transaction, state, code)
                .await
        })
    }

    /// Fetch and normalize the provider's profile. Takes the full [`TokenSet`]
    /// so an OIDC provider may read identity from the id_token rather than a
    /// userinfo endpoint. No default — this is the per-provider code.
    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a>;
}
