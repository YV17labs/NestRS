//! **Auth** templates — the app-side authn/authz adapter (`g auth`).
//!
//! These types are *app* code, not framework code: the framework is generic
//! over the principal (`Claims`) and the policy (`AppAbility`), so every
//! project writes the same eight small files once. They mirror
//! `demo/crates/features/src/{identity,authn,authz}/` — copy that exemplar
//! when extending, don't invent a second shape.

/// The principal. `JwtStrategy<Claims>` deserializes a verified token into it,
/// and `AppAbility` reads it to build the caller's rules.
pub const AUTHN_CLAIMS: &str = r#"use nest_rs::authn::PrincipalIdentity;
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

pub const AUTHN_MOD: &str = r#"mod claims;
mod module;
mod strategy;

pub mod http;

pub use claims::{Claims, Role};
pub use http::AppAuthnHttpModule;
pub use module::AppAuthnModule;
pub use strategy::AppAuthnGuard;
"#;

pub const AUTHN_STRATEGY: &str = r#"use nest_rs::authn::JwtStrategy;

use crate::app_authn::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

/// Bind first, before the ability guard:
/// `#[use_guards(AppAuthnGuard, AppAuthzGuard)]`.
pub type AppAuthnGuard = nest_rs::authn::AuthnGuard<AppJwtStrategy>;
"#;

pub const AUTHN_MODULE: &str = r#"use nest_rs::authn::AuthnModule;
use nest_rs::core::module;

use super::strategy::{AppAuthnGuard, AppJwtStrategy};

#[module(
    imports = [AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AppAuthnGuard],
)]
pub struct AppAuthnModule;
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

pub use module::AppAuthnHttpModule;
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
use crate::app_authn::{Claims, Role};

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
/// boot refusal covers the app that imports `AppAuthnHttpModule`; this covers the
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
             crates/features/src/app_authn/http/ and its AppAuthnHttpModule import and write the real \
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
use crate::app_authn::AppAuthnModule;

#[module(
    imports = [AppAuthnModule],
    providers = [DevTokenAudit, DevOnlyGuard, DevTokenController],
)]
pub struct AppAuthnHttpModule;
"#;

pub const AUTHZ_MOD: &str = r#"mod ability;
mod module;

pub mod http;

pub use ability::AppAbility;
pub use module::AppAuthzModule;

pub use http::{AppAuthzGuard, AppAuthzHttpModule};
"#;

/// The whole policy, in one function. Empty on purpose: the data layer denies
/// every row the ability does not grant, so an app that grants nothing serves
/// nothing — a legible 403, never a silent empty list.
pub const AUTHZ_ABILITY: &str = r#"use nest_rs::authz::{AbilityBuilder, AbilityFactory};
use nest_rs::core::injectable;

use crate::app_authn::Claims;

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
use crate::app_authn::AppAuthnModule;

#[module(
    imports = [AppAuthnModule],
    providers = [AppAbility],
)]
pub struct AppAuthzModule;
"#;

pub const AUTHZ_HTTP_MOD: &str = r#"mod guard;
mod module;

pub use guard::AppAuthzGuard;
pub use module::AppAuthzHttpModule;
"#;

pub const AUTHZ_HTTP_GUARD: &str = r#"use nest_rs::authz::http::AbilityGuard;

use crate::app_authz::AppAbility;

pub type AppAuthzGuard = AbilityGuard<AppAbility>;
"#;

pub const AUTHZ_HTTP_MODULE: &str = r#"use nest_rs::core::module;

use super::guard::AppAuthzGuard;
use crate::app_authz::AppAuthzModule;

#[module(
    imports = [AppAuthzModule],
    providers = [AppAuthzGuard],
)]
pub struct AppAuthzHttpModule;
"#;

// ── authz/graphql/ — the per-operation bridge (`nestrs g graphql`) ──────────
//
// `/graphql` is one endpoint with no guard at the HTTP edge: authn and the
// ability run **in band, per operation**, through a `GraphqlOperationGuard`.
// These three providers are what a resolver's `#[authorize]` / `#[public]`
// posture is enforced against, so a GraphQL adapter without them boots into a
// deny-all fallback that installs no ability at all.

pub const AUTHZ_GRAPHQL_MOD: &str = r#"mod bridge;
mod module;

pub use module::AppAuthzGraphqlModule;
"#;

/// The operation guard: runs the controllers' own chain (`AppAuthnGuard`, then
/// `AppAuthzGuard`) on the GraphQL request, then scopes the operation to the
/// ability it produced — so one policy answers on both transports.
pub const AUTHZ_GRAPHQL_BRIDGE: &str = r#"use nest_rs::authz::graphql::GraphqlAbilityBridge;

use crate::app_authn::AppAuthnGuard;
use crate::app_authz::http::AppAuthzGuard;

pub type AppGraphqlGuard = GraphqlAbilityBridge<AppAuthnGuard, AppAuthzGuard>;
"#;

pub const AUTHZ_GRAPHQL_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::graphql::{GraphqlBatchContext, GraphqlOperationGuard, forward_principal};
use nest_rs::seaorm::graphql::LoaderScope;

use super::bridge::AppGraphqlGuard;
use crate::app_authz::http::AppAuthzHttpModule;
use crate::app_authn::Claims;

#[module(
    imports = [AppAuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn GraphqlOperationGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AppAuthzGraphqlModule;

// Forwards the verified principal into every operation's GraphQL context.
// Anonymous requests pass through untouched, so nothing to gate: the guard that
// attaches the principal is the gate.
forward_principal!(Claims);
"#;

// ── authz/ws/ — the socket-side context (`nestrs g ws`) ────────────────────
//
// A WS upgrade is an HTTP GET, so the gateway reuses the HTTP guards
// (`#[use_guards(AppAuthnGuard, AppAuthzGuard)]`) rather than a bridge of its own.
// What it does need is the `dyn SocketContext` that carries the connection's
// data scope — without it a guarded gateway serves rows nobody scoped, and the
// generated gateway's own SECURITY comment tells the reader to import a module
// nothing was writing.

pub const AUTHZ_WS_MOD: &str = r#"mod module;

pub use module::AppAuthzWsModule;
"#;

pub const AUTHZ_WS_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::seaorm::ws::WsDataContext;
use nest_rs::ws::{SocketContext, WsModule};

use crate::app_authz::http::AppAuthzHttpModule;

#[module(
    imports = [AppAuthzHttpModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AppAuthzWsModule;
"#;

// ── authz/mcp/ — the per-operation bridge (`nestrs g mcp`) ─────────────────
//
// `/mcp` is one endpoint gated in band, per operation, through an
// `McpOperationGuard`. With none registered the endpoint is **deny-all**: every
// tool call answers 401, which is the boot warning `nestrs g mcp` prints. These
// three providers are what turn that into a real posture.

pub const AUTHZ_MCP_MOD: &str = r#"mod bridge;
mod module;

pub use module::AppAuthzMcpModule;
"#;

/// The operation guard: runs the controllers' own chain (`AppAuthnGuard`, then
/// `AppAuthzGuard`) on the MCP request, then installs the ambient `Ability` a tool
/// returns masked rows through — so one policy answers on every transport.
pub const AUTHZ_MCP_BRIDGE: &str = r#"use nest_rs::authz::mcp::McpAbilityBridge;

use crate::app_authn::AppAuthnGuard;
use crate::app_authz::http::AppAuthzGuard;

pub type AppMcpGuard = McpAbilityBridge<AppAuthnGuard, AppAuthzGuard>;
"#;

pub const AUTHZ_MCP_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::mcp::{McpOperationGuard, McpToolContext};
use nest_rs::seaorm::mcp::McpDataContext;

use super::bridge::AppMcpGuard;
use crate::app_authz::http::AppAuthzHttpModule;

#[module(
    imports = [AppAuthzHttpModule],
    providers = [
        AppMcpGuard as dyn McpOperationGuard,
        McpDataContext as dyn McpToolContext,
    ],
)]
pub struct AppAuthzMcpModule;
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
