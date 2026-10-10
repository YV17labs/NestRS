//! **Auth** templates — the app-side authn/authz adapter (`g auth`).
//!
//! App code, not framework code: the framework is generic over the principal
//! (`Claims`) and the policy (`AuthzAbility`). They mirror
//! `demo/crates/features/src/{authn,authz}/`.
//!
//! `authn/module.rs` writes `nest_rs::authn::AuthnModule::for_root(None)`
//! qualified: the product's own module wears the same name (`E0255`).

/// The principal. `JwtStrategy<Claims>` deserializes a verified token into it,
/// and `AuthzAbility` reads it to build the caller's rules.
pub(crate) const AUTHN_CLAIMS: &str = r#"use nest_rs::authn::PrincipalIdentity;
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

pub(crate) const AUTHN_MOD: &str = r#"mod claims;
mod module;
mod strategy;

pub mod http;

pub use claims::{Claims, Role};
pub use http::AuthnHttpModule;
pub use module::AuthnModule;
pub use strategy::AuthnGuard;
"#;

pub(crate) const AUTHN_STRATEGY: &str = r#"use nest_rs::authn::JwtStrategy;

use crate::authn::Claims;

pub type AuthnStrategy = JwtStrategy<Claims>;

/// Bind first, before the ability guard:
/// `#[use_guards(AuthnGuard, AuthzGuard)]`.
pub type AuthnGuard = nest_rs::authn::AuthnGuard<AuthnStrategy>;
"#;

pub(crate) const AUTHN_MODULE: &str = r#"use nest_rs::core::module;

use super::strategy::{AuthnGuard, AuthnStrategy};

#[module(
    imports = [nest_rs::authn::AuthnModule::for_root(None)],
    providers = [AuthnStrategy, AuthnGuard],
)]
pub struct AuthnModule;
"#;

// authn/http/ — the development token route, refusing the boot outside
// `development` / `test`. Deleted the day the real login lands.

pub(crate) const AUTHN_HTTP_MOD: &str = r#"mod audit;
mod controller;
mod guard;
mod module;

pub use module::AuthnHttpModule;
"#;

/// The route the tutorial `curl`s. `#[public]`: the environment check stands in
/// for the credential this route deliberately does not have.
pub(crate) const AUTHN_HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::authn::JwtService;
use nest_rs::http::poem::error::InternalServerError;
use nest_rs::http::poem::web::Json;
use nest_rs::http::poem::Result;
use nest_rs::http::{controller, input, routes};
use uuid::Uuid;

use super::guard::DevOnlyGuard;
use crate::authn::{Claims, Role};

#[input]
#[derive(Debug, Default)]
#[serde(default)]
pub struct DevTokenDto {
    pub sub: Option<Uuid>,
    pub roles: Vec<Role>,
}

#[input]
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
    #[response_header("cache-control", "no-store")]
    #[response_header("pragma", "no-cache")]
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
/// Not on `DevTokenController`: a `#[controller]` registers metadata, never an
/// instance, so its `#[hooks]` could not run (`nest_rs::core::ProviderResidency`).
pub(crate) const AUTHN_HTTP_GUARD: &str = r#"use nest_rs::core::{Layer, injectable};
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

pub(crate) const AUTHN_HTTP_AUDIT: &str = r#"use nest_rs::config::Environment;
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

pub(crate) const AUTHN_HTTP_MODULE: &str = r#"use nest_rs::core::module;

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

pub(crate) const AUTHZ_MOD: &str = r#"mod ability;
mod guard;
mod module;

pub use ability::AuthzAbility;
pub use guard::AuthzGuard;
pub use module::AuthzModule;
"#;

/// The whole policy, in one function. Empty on purpose: an app that grants
/// nothing serves nothing — a legible 403, never a silent empty list.
pub(crate) const AUTHZ_ABILITY: &str = r#"use nest_rs::authz::{AbilityBuilder, AbilityFactory};
use nest_rs::core::injectable;

use crate::authn::Claims;

#[injectable]
#[derive(Default)]
pub struct AuthzAbility;

impl AbilityFactory for AuthzAbility {
    type Actor = Claims;

    /// Every rule this app grants, keyed off the authenticated actor. Nothing
    /// is granted until you add a `can` here — reads return 403 and no row
    /// crosses the data layer.
    ///
    /// `nestrs g resource <name>` prints the grant to paste for the resource it
    /// just generated, with `_ab` renamed `ab`; narrow it with
    /// `.when(|p| p.eq(Column::AuthorId, actor.sub))` to the caller's own rows.
    fn define(&self, _actor: &Claims, _ab: &mut AbilityBuilder) {}

    /// What an *unauthenticated* caller may do, on a `#[public]` route only.
    /// Empty on purpose: nothing is public until you say so here, and a route
    /// you open with `#[public]` still serves no row without a grant — a
    /// `can(Action::Read, …)` narrowed by `.when(…)` to the rows meant for
    /// anyone.
    ///
    /// A `#[public]` route reached with a valid token uses `define` instead —
    /// this branch answers only for the visitor.
    fn define_visitor(&self, _ab: &mut AbilityBuilder) {}
}
"#;

pub(crate) const AUTHZ_MODULE: &str = r#"use nest_rs::core::module;

use super::ability::AuthzAbility;
use super::guard::AuthzGuard;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [AuthzAbility, AuthzGuard],
)]
pub struct AuthzModule;
"#;

/// At the `authz/` root, not under `authz/http/`: `AbilityGuard` answers every
/// transport.
pub(crate) const AUTHZ_GUARD: &str = r#"use nest_rs::authz::AbilityGuard;

use crate::authz::AuthzAbility;

pub type AuthzGuard = AbilityGuard<AuthzAbility>;
"#;

// authz/graphql/ — the per-operation bridge (`nestrs g graphql`); without it a
// GraphQL adapter installs no ability.

pub(crate) const AUTHZ_GRAPHQL_MOD: &str = r#"mod bridge;
mod module;

pub use module::AuthzGraphqlModule;
"#;

/// The operation guard: runs the controllers' own chain (`AuthnGuard`, then
/// `AuthzGuard`) on the GraphQL request, then scopes the operation to the
/// ability it produced — so one policy answers on both transports.
pub(crate) const AUTHZ_GRAPHQL_BRIDGE: &str = r#"use nest_rs::authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;

pub type AuthzGraphqlBridge = GraphqlAbilityBridge<AuthnGuard, AuthzGuard>;
"#;

pub(crate) const AUTHZ_GRAPHQL_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::graphql::{GraphqlBatchContext, GraphqlOperationGuard, forward_principal};
use nest_rs::seaorm::graphql::LoaderScope;

use super::bridge::AuthzGraphqlBridge;
use crate::authz::AuthzModule;
use crate::authn::Claims;

#[module(
    imports = [AuthzModule],
    providers = [
        AuthzGraphqlBridge as dyn GraphqlOperationGuard,
        LoaderScope as dyn GraphqlBatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

// Forwards the verified principal into every operation's GraphQL context.
// Anonymous requests pass through untouched, so nothing to gate: the guard that
// attaches the principal is the gate.
forward_principal!(Claims);
"#;

// authz/ws/ — the `dyn SocketContext` carrying a gateway connection's data
// scope (`nestrs g ws`); the upgrade reuses the HTTP guards.

pub(crate) const AUTHZ_WS_MOD: &str = r#"mod module;

pub use module::AuthzWsModule;
"#;

pub(crate) const AUTHZ_WS_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::seaorm::ws::WsDataContext;
use nest_rs::ws::{SocketContext, WsModule};

use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AuthzWsModule;
"#;

// authz/mcp/ — the per-operation bridge (`nestrs g mcp`); without it `/mcp` is
// deny-all.

pub(crate) const AUTHZ_MCP_MOD: &str = r#"mod bridge;
mod module;

pub use module::AuthzMcpModule;
"#;

/// The operation guard: runs the controllers' own chain (`AuthnGuard`, then
/// `AuthzGuard`) on the MCP request, then installs the ambient `Ability` a tool
/// returns masked rows through — so one policy answers on every transport.
pub(crate) const AUTHZ_MCP_BRIDGE: &str = r#"use nest_rs::authz::mcp::McpAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;

pub type AuthzMcpBridge = McpAbilityBridge<AuthnGuard, AuthzGuard>;
"#;

pub(crate) const AUTHZ_MCP_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::mcp::{McpOperationGuard, McpToolContext};
use nest_rs::seaorm::mcp::McpDataContext;

use super::bridge::AuthzMcpBridge;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [
        AuthzMcpBridge as dyn McpOperationGuard,
        McpDataContext as dyn McpToolContext,
    ],
)]
pub struct AuthzMcpModule;
"#;

/// Appended to the committed `.env`, which every environment reads: where the
/// secret lives, never the secret.
pub(crate) const ENV_AUTHN: &str = r#"
# JWT verification (`nestrs g auth`). The HS256 secret is not in this file, which
# every environment reads: `.env.local` (git-ignored) holds yours, and `.env.test`
# the suites', read only under {{env_prefix}}_ENV=test. `nestrs g auth` drew each at
# random; draw another with `openssl rand -hex 32`. A holder of the secret can
# also MINT tokens, so every deployed environment sets its own
# `{{env_prefix}}_AUTHN__SECRET` (or `_FILE`) through the real environment, or switches to
# EdDSA: `{{env_prefix}}_AUTHN__PRIVATE_KEY` and `{{env_prefix}}_AUTHN__PUBLIC_KEY` on the issuing
# app, `{{env_prefix}}_AUTHN__PUBLIC_KEY` alone on the resource servers — each inline, or as
# a path in its `_FILE` form. A secret beside either key fails the boot, so
# switching to EdDSA means deleting the secret from `.env.local` and `.env.test`.
# (A config pinned in code stops these files being read for the namespace.)
"#;

/// Appended to the git-ignored `.env.local`: the developer's own key.
pub(crate) const ENV_LOCAL_AUTHN: &str = r#"
# HS256 secret of this machine's development runs (`nestrs g auth`), drawn at
# random. Git-ignored: it never leaves this machine.
{{env_prefix}}_AUTHN__SECRET={{secret}}
"#;

/// Appended to the committed `.env.test`: read only under a declared `test`, so
/// the key mints only tokens a test process accepts.
pub(crate) const ENV_TEST_AUTHN: &str = r#"
# HS256 secret the test suites sign and verify with (`nestrs g auth`), drawn at
# random. Committed: the cascade reads this file only under {{env_prefix}}_ENV=test, so
# a teammate and CI run the suites with no copy step, and a token signed with
# it passes nowhere else.
{{env_prefix}}_AUTHN__SECRET={{secret}}
"#;

/// Appended to `.env.example`: what a teammate's `.env.local` needs.
pub(crate) const ENV_EXAMPLE_AUTHN: &str = r#"
# JWT verification (`nestrs g auth`): an HS256 secret of your own, drawn with
# `openssl rand -hex 32`.
# {{env_prefix}}_AUTHN__SECRET=
"#;
