//! **Auth** templates — the app-side authn/authz adapter (`g auth`).
//!
//! These types are *app* code, not framework code: the framework is generic
//! over the principal (`Claims`) and the policy (`AppAbility`), so every
//! project writes the same eight small files once. They mirror
//! `demo/crates/features/src/{identity,authn,authz}/` — copy that exemplar
//! when extending, don't invent a second shape.

pub const IDENTITY_MOD: &str = r#"mod claims;

pub use claims::{Claims, Role};
"#;

/// The principal. `JwtStrategy<Claims>` deserializes a verified token into it,
/// and `AppAbility` reads it to build the caller's rules.
pub const IDENTITY_CLAIMS: &str = r#"use nest_rs_authn::PrincipalIdentity;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

/// The JWT payload this app verifies. Every field is read from the token, so
/// whatever mints those tokens has to put them there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<Uuid>,
    pub roles: Vec<Role>,
    pub exp: u64,
}

impl Claims {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}

// Puts `actor_id` on the request span, so every downstream event — denials
// included — is attributable without threading the id through each call site.
impl PrincipalIdentity for Claims {
    fn actor_id(&self) -> Option<String> {
        self.sub.map(|sub| sub.to_string())
    }
}
"#;

pub const AUTHN_MOD: &str = r#"mod module;
mod strategy;

pub use module::AuthnModule;
pub use strategy::AuthnGuard;
"#;

pub const AUTHN_STRATEGY: &str = r#"use nest_rs_authn::JwtStrategy;

use crate::identity::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

/// Bind first, before the ability guard:
/// `#[use_guards(AuthnGuard, AuthzGuard)]`.
pub type AuthnGuard = nest_rs_authn::AuthnGuard<AppJwtStrategy>;
"#;

pub const AUTHN_MODULE: &str = r#"use nest_rs_core::module;

use super::strategy::{AppJwtStrategy, AuthnGuard};

#[module(
    imports = [nest_rs_authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthnGuard],
)]
pub struct AuthnModule;
"#;

pub const AUTHZ_MOD: &str = r#"mod ability;
mod module;

pub mod http;

pub use ability::AppAbility;
pub use module::AuthzModule;

pub use http::{AuthzGuard, AuthzHttpModule};
"#;

/// The whole policy, in one function. Empty on purpose: the data layer denies
/// every row the ability does not grant, so an app that grants nothing serves
/// nothing — a legible 403, never a silent empty list.
pub const AUTHZ_ABILITY: &str = r#"use nest_rs_authz::{AbilityBuilder, AbilityFactory};
use nest_rs_core::injectable;

use crate::identity::Claims;

#[injectable]
#[derive(Default)]
pub struct AppAbility;

impl AbilityFactory for AppAbility {
    type Actor = Claims;

    /// Every rule this app grants, keyed off the authenticated actor. Nothing
    /// is granted until you add a `can` here — reads return 403 and no row
    /// crosses the data layer.
    ///
    /// ```ignore
    /// use nest_rs_authz::Action;
    /// use crate::posts as post;
    ///
    /// ab.can(Action::Read, post::Entity);
    /// ab.can(Action::Manage, post::Entity)
    ///     .when(|p| p.eq(post::Column::AuthorId, actor.sub));
    /// ```
    ///
    /// `nestrs g resource <name>` prints the two lines to paste for the
    /// resource it just generated.
    fn define(&self, _actor: &Claims, _ab: &mut AbilityBuilder) {}

    /// What an *unauthenticated* caller may do, on a `#[public]` route only.
    /// Empty on purpose: nothing is public until you say so here, and a route
    /// you open with `#[public]` still serves no row without a grant.
    ///
    /// ```ignore
    /// use nest_rs_authz::Action;
    /// use crate::posts as post;
    ///
    /// ab.can(Action::Read, post::Entity)
    ///     .when(|p| p.eq(post::Column::Published, true));
    /// ```
    ///
    /// A `#[public]` route reached with a valid token uses `define` instead —
    /// this branch answers only for the visitor.
    fn define_visitor(&self, _ab: &mut AbilityBuilder) {}
}
"#;

pub const AUTHZ_MODULE: &str = r#"use nest_rs_core::module;

use super::ability::AppAbility;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [AppAbility],
)]
pub struct AuthzModule;
"#;

pub const AUTHZ_HTTP_MOD: &str = r#"mod guard;
mod module;

pub use guard::AuthzGuard;
pub use module::AuthzHttpModule;
"#;

pub const AUTHZ_HTTP_GUARD: &str = r#"use nest_rs_authz::http::AbilityGuard;

use crate::authz::AppAbility;

pub type AuthzGuard = AbilityGuard<AppAbility>;
"#;

pub const AUTHZ_HTTP_MODULE: &str = r#"use nest_rs_core::module;

use super::guard::AuthzGuard;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [AuthzGuard],
)]
pub struct AuthzHttpModule;
"#;

/// Appended to the committed `.env`. HS256 needs ≥ 32 bytes or the app refuses
/// to boot; this placeholder is deliberately obvious so nobody ships it.
pub const ENV_AUTHN: &str = r#"
# JWT verification (`nestrs g auth`). HS256 shared secret — a holder can also
# MINT tokens, so this value is a local-development placeholder only: set a
# real `NESTRS_AUTHN__SECRET` through the real environment in every deployed
# environment, or switch to EdDSA (`NESTRS_AUTHN__PRIVATE_KEY` on the issuing
# app, `NESTRS_AUTHN__PUBLIC_KEY` on the resource servers).
NESTRS_AUTHN__SECRET=dev-only-insecure-secret-change-me-32b
"#;
