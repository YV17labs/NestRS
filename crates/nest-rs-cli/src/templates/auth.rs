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
pub const IDENTITY_CLAIMS: &str = r#"use nest_rs::authn::PrincipalIdentity;
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

pub const AUTHN_STRATEGY: &str = r#"use nest_rs::authn::JwtStrategy;

use crate::identity::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

/// Bind first, before the ability guard:
/// `#[use_guards(AuthnGuard, AuthzGuard)]`.
pub type AuthnGuard = nest_rs::authn::AuthnGuard<AppJwtStrategy>;
"#;

pub const AUTHN_MODULE: &str = r#"use nest_rs::core::module;

use super::strategy::{AppJwtStrategy, AuthnGuard};

#[module(
    imports = [nest_rs::authn::AuthnModule::for_root(None)],
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
pub const AUTHZ_ABILITY: &str = r#"use nest_rs::authz::{AbilityBuilder, AbilityFactory};
use nest_rs::core::injectable;

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
    /// use nest_rs::authz::Action;
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
    /// use nest_rs::authz::Action;
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

pub const AUTHZ_MODULE: &str = r#"use nest_rs::core::module;

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

pub const AUTHZ_HTTP_GUARD: &str = r#"use nest_rs::authz::http::AbilityGuard;

use crate::authz::AppAbility;

pub type AuthzGuard = AbilityGuard<AppAbility>;
"#;

pub const AUTHZ_HTTP_MODULE: &str = r#"use nest_rs::core::module;

use super::guard::AuthzGuard;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [AuthzGuard],
)]
pub struct AuthzHttpModule;
"#;

// ── authz/graphql/ — the per-operation bridge (`nestrs g graphql`) ──────────
//
// `/graphql` is one endpoint with no guard at the HTTP edge: authn and the
// ability run **in band, per operation**, through a `GraphqlOperationGuard`.
// These three providers are what a resolver's `#[authorize]` / `#[public]`
// posture is enforced against, so a GraphQL adapter without them boots into a
// deny-all fallback that installs no ability at all.

pub const AUTHZ_GRAPHQL_MOD: &str = r#"mod bridge;
mod guard;
mod module;

pub use module::AuthzGraphqlModule;
"#;

/// The operation guard: runs the controllers' own chain (`AuthnGuard`, then
/// `AuthzGuard`) on the GraphQL request, then scopes the operation to the
/// ability it produced — so one policy answers on both transports.
pub const AUTHZ_GRAPHQL_BRIDGE: &str = r#"use nest_rs::authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::http::AuthzGuard;

pub type AppGraphqlGuard = GraphqlAbilityBridge<AuthnGuard, AuthzGuard>;
"#;

/// The marker that owns the seeded `Claims` context entry. It is not a guard
/// chain member: `forward_principal!` gates the principal it forwards on this
/// type being reachable, which turns "did the app import the GraphQL authz
/// module?" into a boot-time access-graph answer instead of a null at run time.
pub const AUTHZ_GRAPHQL_GUARD: &str = r#"use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
pub struct GraphqlAuthnGuard;
"#;

pub const AUTHZ_GRAPHQL_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::graphql::{GraphqlBatchContext, GraphqlOperationGuard};
use nest_rs::seaorm::graphql::LoaderScope;

use super::bridge::AppGraphqlGuard;
use super::guard::GraphqlAuthnGuard;
use crate::authz::http::AuthzHttpModule;
use crate::identity::Claims;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn GraphqlOperationGuard,
        GraphqlAuthnGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

// Forwards the verified principal into every operation's GraphQL context,
// gated on `GraphqlAuthnGuard` being reachable from the running app.
nest_rs::graphql::forward_principal!(Claims, GraphqlAuthnGuard);
"#;

// ── authz/ws/ — the socket-side context (`nestrs g ws`) ────────────────────
//
// A WS upgrade is an HTTP GET, so the gateway reuses the HTTP guards
// (`#[use_guards(AuthnGuard, AuthzGuard)]`) rather than a bridge of its own.
// What it does need is the `dyn SocketContext` that carries the connection's
// data scope — without it a guarded gateway serves rows nobody scoped, and the
// generated gateway's own SECURITY comment tells the reader to import a module
// nothing was writing.

pub const AUTHZ_WS_MOD: &str = r#"mod module;

pub use module::AuthzWsModule;
"#;

pub const AUTHZ_WS_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::seaorm::ws::WsDataContext;
use nest_rs::ws::{SocketContext, WsModule};

use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AuthzWsModule;
"#;

// ── authz/mcp/ — the per-operation bridge (`nestrs g mcp`) ─────────────────
//
// `/mcp` is one endpoint gated in band, per operation, through an
// `McpOperationGuard`. With none registered the endpoint is **deny-all**: every
// tool call answers 401, which is the boot warning `nestrs g mcp` prints. These
// three providers are what turn that into a real posture.

pub const AUTHZ_MCP_MOD: &str = r#"mod bridge;
mod module;

pub use module::AuthzMcpModule;
"#;

/// The operation guard: runs the controllers' own chain (`AuthnGuard`, then
/// `AuthzGuard`) on the MCP request, then installs the ambient `Ability` a tool
/// returns masked rows through — so one policy answers on every transport.
pub const AUTHZ_MCP_BRIDGE: &str = r#"use nest_rs::authz::mcp::McpAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::http::AuthzGuard;

pub type AppMcpGuard = McpAbilityBridge<AuthnGuard, AuthzGuard>;
"#;

pub const AUTHZ_MCP_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::mcp::{McpOperationGuard, McpToolContext};
use nest_rs::seaorm::mcp::McpDataContext;

use super::bridge::AppMcpGuard;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppMcpGuard as dyn McpOperationGuard,
        McpDataContext as dyn McpToolContext,
    ],
)]
pub struct AuthzMcpModule;
"#;

/// Appended to the committed `.env`. HS256 needs ≥ 32 bytes or the app refuses
/// to boot; this placeholder is deliberately obvious so nobody ships it.
pub const ENV_AUTHN: &str = r#"
# JWT verification (`nestrs g auth`). HS256 shared secret — a holder can also
# MINT tokens, so this value is a local-development placeholder only: set a
# real `{{env_prefix}}_AUTHN__SECRET` through the real environment in every deployed
# environment, or switch to EdDSA (`{{env_prefix}}_AUTHN__PRIVATE_KEY` on the issuing
# app, `{{env_prefix}}_AUTHN__PUBLIC_KEY` on the resource servers).
{{env_prefix}}_AUTHN__SECRET=dev-only-insecure-secret-change-me-32b
"#;
