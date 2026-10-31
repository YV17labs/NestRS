//! Authentication for nestrs — establishing *who* the caller is, and nothing
//! else.
//!
//! [`Strategy`] turns a request into a principal; [`AuthnGuard`] runs one,
//! records the resulting [`actor_id`](PrincipalIdentity::actor_id) as the audit
//! identity every downstream event inherits, and makes the
//! [`tenant_id`](PrincipalIdentity::tenant_id) it names ambient for the request
//! and every job it pushes. [`JwtService`] signs and verifies —
//! against a secret, an EdDSA key, or the JWK Set an external issuer publishes;
//! [`hash_password`] / [`verify_password`] cover the local-credential case.
//! The product wiring that binds them — the concrete claims type, the user
//! lookup, the lockout policy — belongs to the consuming application (in this
//! repo, `demo/crates/features`), whose claims type derives what the guard
//! reads off it with [`#[derive(PrincipalIdentity)]`](macro@PrincipalIdentity).
//!
//! **What lives elsewhere.** The RFC 6749 §1.1 roles have their own crates, so
//! an app links the one it plays:
//!
//! | Concern | Crate |
//! |---|---|
//! | obtaining a token from someone else's authorization server | `nest-rs-oauth-client` |
//! | issuing tokens — §5.2 error codes, §2.3.1 client authentication | `nest-rs-oauth-server` |
//! | RFC 9728 discovery — what this deployment is, and where to get a token | `nest-rs-oauth-resource` |
//! | social login behind a discovered provider contract | `nest-rs-social` |

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — principal resolution: strategies, credential
/// verification, the guard's authentication outcome, and the refusals of a
/// development-only route.
pub const TARGET: &str = "nest_rs::authn";

mod algorithm;
mod binding;
mod config;
mod credentials;
mod development;
mod error;
mod guard;
mod jwk_set;
mod jwks;
mod module;
mod password;
mod principal;
pub mod scope;
mod service;
mod strategies;
mod strategy;

pub use algorithm::Algorithm;
pub use config::AuthnConfig;
pub use credentials::{basic_credentials, bearer_token};
pub use development::DevelopmentOnlyGuard;
pub use error::{AuthError, CredentialError, PasswordError, UnknownAlgorithmError};
pub use guard::AuthnGuard;
pub use jwks::{
    JWKS_CONNECT_TIMEOUT, JWKS_DEFAULT_MAX_AGE, JWKS_FETCH_TIMEOUT, JWKS_MAX_AGE_CEILING,
    JWKS_MAX_BYTES, JWKS_REFRESH_FLOOR, JWKS_STALE_CEILING,
};
pub use module::{AuthnModule, AuthnSetup};
/// Derives [`PrincipalIdentity`](trait@PrincipalIdentity) and `AmbientValue`
/// for a claims struct, each role read off the field `#[principal(…)]` marks.
///
/// - `#[principal(actor)]`, required: the subject the verified credential
///   names, an access token's `sub`. `actor_id` answers the text its value
///   prints — a `String`, a `Uuid`, a `Subject` enum of your own with its
///   `Display` — or, on an `Option`, `None` when it holds none.
/// - `#[principal(tenant)]`: the tenant the principal acts within, read the
///   same way. Unmarked, `tenant_id` answers `None`.
/// - `#[principal(scopes)]`: the scopes the credential was granted, a
///   `Vec<String>`, which `scopes` answers whole — empty when the token carried
///   none. An `Option` is refused: it would read a token sent without its
///   `scope` claim as a credential scope is no dimension of, to which every
///   scoped rule applies in full. Unmarked, `scopes` answers `None`.
///
/// A role is read by the field's type, never its spelling, so an alias of
/// `Option` is an `Option`. Each role is played by one field, and one field may
/// play several (`#[principal(actor, tenant)]`). `AmbientValue` keeps its
/// default carry: the claims travel with the request and whatever continues
/// it, never into a job's envelope, which carries the actor and the tenant
/// alone. A generic struct, an enum and a union are refused; such a principal,
/// or one narrowing what it carries, writes both impls by hand.
///
/// ```
/// use nest_rs_authn::PrincipalIdentity;
/// use nest_rs_core::AmbientValue;
///
/// #[derive(Clone, PrincipalIdentity)]
/// struct Claims {
///     #[principal(actor)]
///     sub: String,
///     #[principal(tenant)]
///     org_id: Option<u64>,
///     #[principal(scopes)]
///     scope: Vec<String>,
///     roles: Vec<String>,
/// }
///
/// /// What a `Strategy` answers with.
/// fn principal<P: PrincipalIdentity + AmbientValue + Clone>(_: &P) {}
///
/// let claims = Claims {
///     sub: "ada".into(),
///     org_id: Some(7),
///     scope: vec!["posts:read".into()],
///     roles: Vec::new(),
/// };
/// principal(&claims);
/// assert_eq!(claims.actor_id().as_deref(), Some("ada"));
/// assert_eq!(claims.tenant_id().as_deref(), Some("7"));
/// assert_eq!(claims.scopes(), Some(&["posts:read".to_owned()][..]));
/// ```
///
/// # Expands to
///
/// `impl PrincipalIdentity` with `actor_id`, and `tenant_id` and `scopes` for
/// the roles a field plays, each reading its field; and an empty
/// `impl AmbientValue`.
pub use nest_rs_authn_macros::PrincipalIdentity;
pub use password::{burn_verify, hash_password, verify_password};
pub use principal::PrincipalIdentity;
pub use service::{JwtKey, JwtOptions, JwtService};
pub use strategies::JwtStrategy;
pub use strategy::{AUTHENTICATE_TIMEOUT, Strategy};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::binding::ResourceBinding;
    pub use crate::guard::{Verdict, authenticate};
    pub use crate::principal::{
        Actor, ActorField, ActorRefusal, IdentifierAnswer, IdentifierProbe, ScopeList, Tenant,
        TenantField, TenantRefusal, scope_list,
    };
}
