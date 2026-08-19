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
use nest_rs::resource::wire_enum;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// `Role` crosses the wire inside every DTO that names it, so it carries the
// wire derives rather than serde alone — one decorator, no second manifest line.
#[wire_enum]
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

pub mod http;

pub use http::AuthnHttpModule;
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

// ── authn/http/ — the development token route ───────────────────────────────
//
// Every guarded route needs a bearer token, and until an app writes its real
// login there is nothing that mints one. The gap used to be filled by the docs
// telling a reader to hand-sign an HS256 token in a shell heredoc — a
// cryptography exercise on page five of a tutorial, for a framework whose whole
// claim is that it carries this kind of work.
//
// So `g auth` writes the route instead, and makes it impossible to ship: the
// module refuses the boot outside `development` / `test`, by name, before a
// single request is served. Delete `authn/http/` the day the real login lands —
// nothing else references it.

pub const AUTHN_HTTP_MOD: &str = r#"mod audit;
mod controller;
mod guard;
mod module;

pub use module::AuthnHttpModule;
"#;

/// The route the tutorial `curl`s. `#[public]` because a caller with no token
/// is exactly who asks for one, and the environment check is what stands in for
/// the credential this route deliberately does not have.
pub const AUTHN_HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::authn::JwtService;
use nest_rs::http::poem::error::InternalServerError;
use nest_rs::http::poem::web::Json;
use nest_rs::http::poem::Result;
use nest_rs::http::{controller, input, routes};
use uuid::Uuid;

use super::guard::DevOnlyGuard;
use crate::identity::{Claims, Role};

#[input]
#[derive(Debug, Default)]
#[serde(default)]
pub struct DevTokenDto {
    pub sub: Option<Uuid>,
    pub roles: Vec<Role>,
}

#[input]
#[derive(Debug)]
pub struct DevTokenResponseDto {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[controller(path = "/auth")]
#[use_guards(DevOnlyGuard)]
pub struct DevTokenController {
    #[inject]
    jwt: Arc<JwtService>,
}

#[routes]
impl DevTokenController {
    #[post("/dev-token")]
    #[public]
    #[api(summary = "Mint a development-only bearer token")]
    async fn dev_token(&self, body: Json<DevTokenDto>) -> Result<Json<DevTokenResponseDto>> {
        let DevTokenDto { sub, roles } = body.0;
        let claims = Claims {
            sub: sub.or_else(|| Some(Uuid::now_v7())),
            roles: if roles.is_empty() { vec![Role::User] } else { roles },
            exp: self.jwt.expiry(),
        };
        let access_token = self.jwt.sign(&claims).map_err(InternalServerError)?;
        Ok(Json(DevTokenResponseDto {
            access_token,
            token_type: "Bearer".into(),
            expires_in: self.jwt.ttl_secs(),
        }))
    }
}
"#;

/// The boot refusal, on a provider of its own.
///
/// It does **not** live on `DevTokenController`: a `#[controller]` registers
/// metadata, never an instance, so a `#[hooks]` block on it could only be
/// skipped at boot — which is why the framework refuses that composition at
/// compile time (`nest_rs::core::ProviderResidency`). Same shape as the framework's
/// own `SoftDeleteAudit`: an `#[injectable]` whose only job is to refuse.
pub const AUTHN_HTTP_GUARD: &str = r#"use nest_rs::core::{Layer, injectable};
use nest_rs::guards::{Denial, Guard, HttpGuard};
use nest_rs::http::async_trait;
use nest_rs::http::poem::Request;

use super::audit::is_development;

/// Refuses every request unless this is a development or test process. The
/// boot refusal covers the app that imports `AuthnHttpModule`; this covers the
/// route wherever it is mounted from.
#[injectable]
#[derive(Default)]
pub struct DevOnlyGuard;

impl Layer for DevOnlyGuard {}

#[async_trait]
impl Guard for DevOnlyGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        if is_development() {
            return Ok(());
        }
        Err(Denial::forbidden("development-only route"))
    }
}

impl HttpGuard for DevOnlyGuard {}
"#;

pub const AUTHN_HTTP_AUDIT: &str = r#"use nest_rs::config::Environment;
use nest_rs::core::anyhow::{anyhow, Result};
use nest_rs::core::{hooks, injectable};

/// Development and test only. Absence answers `false`: `Environment::declared()`
/// is `None` until somebody sets the variable, so a process nobody told is not
/// a development one.
pub fn is_development() -> bool {
    matches!(
        Environment::declared(),
        Some(Environment::Development | Environment::Test)
    )
}

#[injectable]
#[derive(Default)]
pub struct DevTokenAudit;

#[hooks]
impl DevTokenAudit {
    #[on_module_init]
    async fn refuse_outside_development(&self) -> Result<()> {
        if is_development() {
            return Ok(());
        }
        Err(anyhow!(
            "DevTokenController mints unauthenticated bearer tokens, and {} is not set to \
             `development` or `test` (it reads {:?}). Set it for a development run, or delete \
             crates/features/src/authn/http/ and its AuthnHttpModule import and write the real \
             login route in its place.",
            Environment::var_name(),
            std::env::var(Environment::var_name()).ok(),
        ))
    }
}
"#;

pub const AUTHN_HTTP_MODULE: &str = r#"use nest_rs::core::module;

use super::audit::DevTokenAudit;
use super::controller::DevTokenController;
use super::guard::DevOnlyGuard;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [DevTokenAudit, DevOnlyGuard, DevTokenController],
)]
pub struct AuthnHttpModule;
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
