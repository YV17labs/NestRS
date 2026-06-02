//! The [`Strategy`] trait — how a request becomes an authenticated identity.
//!
//! A strategy is an ordinary `#[injectable]` provider (so it can inject a
//! [`JwtService`](crate::JwtService), an HTTP client, a user repository) that
//! implements one method. [`AuthGuard<S>`](crate::AuthGuard) drives it per route,
//! mirroring NestJS's `AuthGuard('name')` selecting a Passport strategy.
//!
//! There is no `#[strategy]` macro: `#[injectable]` plus this trait is the entire
//! surface, so a macro would generate nothing. One is warranted only if a real
//! boilerplate pattern emerges.

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use nestrs_core::injectable;
use poem::{http::header, Request, Response};
use serde::de::DeserializeOwned;

use crate::error::AuthError;
use crate::jwt::JwtService;

/// What a [`Strategy`] decided about a request.
///
/// The two arms are why one trait serves both stateless and redirect-based
/// schemes: a bearer strategy always [`Authenticated`](Outcome::Authenticated) or
/// errors, while an OAuth strategy [`Challenge`](Outcome::Challenge)s the browser
/// with a redirect to the identity provider on the initiating request and
/// authenticates on the callback.
pub enum Outcome<P> {
    /// Identity established. [`AuthGuard`](crate::AuthGuard) inserts `P` into the
    /// request extensions for downstream guards (e.g. `AbilityGuard`) and the
    /// `Ctx<P>` extractor to read.
    Authenticated(P),
    /// The client must act before it can be authenticated — typically a `302` to
    /// an OAuth provider, or a `401` challenge. The guard short-circuits the
    /// request with this response.
    Challenge(Response),
}

/// Turns a request into an authenticated principal, or says why it cannot.
///
/// Bind it to routes with `#[use_guards(AuthGuard<MyStrategy>)]` (usually via a
/// `type` alias, like `AbilityGuard`).
#[async_trait]
pub trait Strategy: Send + Sync + 'static {
    /// The authenticated caller this strategy produces. Inserted into the request
    /// on success, so downstream code reads it back with `Ctx<Self::Principal>`.
    type Principal: Clone + Send + Sync + 'static;

    /// Inspect the request and decide. The request is borrowed mutably so a
    /// strategy may attach scratch state; the principal itself is attached by
    /// [`AuthGuard`](crate::AuthGuard), not here.
    async fn authenticate(&self, req: &mut Request) -> Result<Outcome<Self::Principal>, AuthError>;
}

/// Pull the token out of an `Authorization: Bearer <token>` header, if present
/// and non-empty. The building block of any bearer/JWT [`Strategy`].
pub fn bearer_token(req: &Request) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then_some(token)
}

/// Pull `(client_id, client_secret)` out of an `Authorization: Basic <base64>`
/// header (RFC 7617), if present and well-formed. The building block of HTTP Basic
/// schemes — chiefly OAuth2 client authentication (RFC 6749 §2.3.1), the
/// RFC-preferred way for a confidential client to authenticate at the token
/// endpoint. The decoded `id:secret` is split on the **first** colon, so a secret
/// may itself contain colons. Symmetric to [`bearer_token`].
pub fn basic_credentials(req: &Request) -> Option<(String, String)> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?.trim();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    Some((id.to_owned(), secret.to_owned()))
}

/// The ready **bearer-JWT** strategy — the framework's `passport-jwt`. Generic over
/// the claims type `C` it verifies into and hands on as the principal: pull the
/// `Authorization: Bearer` token, verify it with the injected [`JwtService`], and
/// authenticate as the decoded `C`. Because it is fully generic (no business
/// logic), an app binds it by *choosing `C`* and aliasing the guard — there is no
/// hand-written strategy to maintain:
///
/// ```ignore
/// pub type AuthGuard = nestrs_authn::AuthGuard<nestrs_authn::JwtStrategy<MyClaims>>;
/// ```
///
/// When an app genuinely needs custom authentication (a revocation check, mapping
/// claims to a richer principal, a non-JWT scheme), it writes its **own**
/// [`Strategy`] instead — this type only covers the standard case, and the trait is
/// the escape hatch.
#[injectable]
pub struct JwtStrategy<C: Send + Sync + 'static> {
    #[inject]
    jwt: Arc<JwtService>,
    _claims: PhantomData<C>,
}

#[async_trait]
impl<C: DeserializeOwned + Clone + Send + Sync + 'static> Strategy for JwtStrategy<C> {
    type Principal = C;

    async fn authenticate(&self, req: &mut Request) -> Result<Outcome<C>, AuthError> {
        let token = bearer_token(req).ok_or(AuthError::MissingCredentials)?;
        let claims: C = self.jwt.verify(token)?;
        Ok(Outcome::Authenticated(claims))
    }
}
