//! **Auth** templates — the app-side authn/authz adapter (`g auth`).
//!
//! App code, not framework code: the framework is generic over the principal
//! (`Claims`) and the policy (`AuthzAbility`). They mirror
//! `demo/crates/features/src/{authn,authz}/`.
//!
//! `authn/module.rs` writes `nest_rs::authn::AuthnModule` qualified: the
//! product's own module wears the same name (`E0255`).

/// The principal. `JwtStrategy<Claims>` deserializes a verified token into it,
/// and `AuthzAbility` reads it to build the caller's rules.
pub(crate) const AUTHN_CLAIMS: &str = r#"use std::fmt;

use nest_rs::authn::PrincipalIdentity;
use nest_rs::core::wire_enum;
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

/// Who a token speaks for: its `sub` claim, which RFC 9068 §2.2 requires in
/// every access token — a user, or a client acting on its own behalf (the
/// `client_credentials` grant), named by its client id.
///
/// One string on the wire, read back by its shape: a UUID is a user. So the
/// registry of your clients must refuse a client id a UUID parses, or a client
/// could pass for a user (RFC 9068 §5): with `nest_rs::oauth::server`, check
/// `OAuthServerConfig`'s clients in an `#[on_module_init]` hook of yours and
/// refuse with `ConfigError::validation`, since this scheme is yours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Subject {
    User(Uuid),
    Client(String),
}

impl Subject {
    /// The user this token speaks for, `None` for a client — what a route
    /// writing a user's row reads.
    pub fn user_id(&self) -> Option<Uuid> {
        match self {
            Self::User(id) => Some(*id),
            Self::Client(_) => None,
        }
    }
}

impl From<String> for Subject {
    fn from(sub: String) -> Self {
        match Uuid::try_parse(&sub) {
            Ok(id) => Self::User(id),
            Err(_) => Self::Client(sub),
        }
    }
}

impl From<Subject> for String {
    fn from(sub: Subject) -> Self {
        sub.to_string()
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(id) => id.fmt(f),
            Self::Client(id) => f.write_str(id),
        }
    }
}

/// The JWT payload this app verifies. Every field is read from the token, so
/// whatever mints those tokens has to put them there; a token without a `sub`
/// is refused. `exp` is the signer's: `JwtService` stamps it, and the verifier
/// refuses a token past it.
// `PrincipalIdentity` makes the claims the principal: the authentication guard
// puts `sub` on the request span as `actor_id`, so every downstream event —
// denials included — is attributable, and attaches the claims for the rest of
// the request, where `Ambient<Claims>` in a handler on every edge reads them back.
// A tenant claim takes `#[principal(tenant)]`, a `scope` claim
// `#[principal(scopes)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PrincipalIdentity)]
pub struct Claims {
    #[principal(actor)]
    pub sub: Subject,
    pub roles: Vec<Role>,
}

impl Claims {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}
"#;

pub(crate) const AUTHN_MOD: &str = r#"mod claims;
mod module;
mod strategy;

pub mod http;

pub use claims::{Claims, Role, Subject};
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
    imports = [nest_rs::authn::AuthnModule],
    providers = [AuthnStrategy, AuthnGuard],
)]
pub struct AuthnModule;
"#;

// authn/http/ — the development token route, behind the framework's
// `DevelopmentOnlyGuard`, which refuses the request and the boot outside
// `development` / `test`. Deleted the day the real login lands.

pub(crate) const AUTHN_HTTP_MOD: &str = r#"mod controller;
mod module;

pub use module::AuthnHttpModule;
"#;

/// The route the tutorial `curl`s. `#[public]`: the environment check stands in
/// for the credential this route deliberately does not have. Only the handler
/// is the app's: it builds the app's own `Claims`, and answers RFC 6749 §5.1's
/// response, which the framework renders.
pub(crate) const AUTHN_HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::authn::{DevelopmentOnlyGuard, JwtService};
use nest_rs::core::input;
use nest_rs::http::{Json, Result, controller, routes};
use nest_rs::oauth::server::AccessTokenResponse;
use uuid::Uuid;

use crate::authn::{Claims, Role, Subject};

#[input]
#[derive(Debug, Default)]
#[serde(default)]
pub struct DevTokenDto {
    pub sub: Option<Uuid>,
    pub roles: Vec<Role>,
}

#[controller(path = "/auth")]
#[use_guards(DevelopmentOnlyGuard)]
pub struct DevTokenController {
    #[inject]
    jwt: Arc<JwtService>,
}

#[routes]
impl DevTokenController {
    #[post("/dev-token")]
    #[public]
    #[api(summary = "Mint a development-only bearer token")]
    async fn dev_token(&self, body: Json<DevTokenDto>) -> Result<AccessTokenResponse> {
        let DevTokenDto { sub, roles } = body.0;
        let claims = Claims {
            sub: Subject::User(sub.unwrap_or_else(Uuid::now_v7)),
            roles: if roles.is_empty() { vec![Role::User] } else { roles },
        };
        Ok(AccessTokenResponse::bearer(
            self.jwt.sign(&claims)?,
            self.jwt.ttl_secs(),
        ))
    }
}
"#;

pub(crate) const AUTHN_HTTP_MODULE: &str = r#"use nest_rs::authn::DevelopmentOnlyGuard;
use nest_rs::core::module;

use super::controller::DevTokenController;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [DevelopmentOnlyGuard, DevTokenController],
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
pub struct AuthzAbility;

impl AbilityFactory for AuthzAbility {
    type Actor = Claims;

    /// Every rule this app grants, keyed off the authenticated actor. Nothing
    /// is granted until you add a `can` here — reads return 403 and no row
    /// crosses the data layer.
    ///
    /// `nestrs g resource <name>` prints the grant to paste for the resource it
    /// just generated, with `_ab` renamed `ab`; narrow it to a user's own rows
    /// with `.when(|p| p.eq(Column::AuthorId, user_id))` under
    /// `if let Some(user_id) = actor.sub.user_id()` — a client owns none.
    fn define(&self, _actor: &Claims, _ab: &mut AbilityBuilder) {}

    /// What an *unauthenticated* caller may do, on what `#[public(Action, Entity)]`
    /// opens. Empty on purpose: nothing is public until you say so here, and a
    /// route naming its read still serves no row without a grant — a
    /// `can(Action::Read, …)` narrowed by `.when(…)` to the rows meant for
    /// anyone.
    ///
    /// Such a route reached with a valid token uses `define` instead — this branch
    /// answers only for the visitor.
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

// authz/graphql/ — the guards at `/graphql`'s request (`nestrs g graphql`);
// without them a GraphQL adapter attaches no ability.

pub(crate) const AUTHZ_GRAPHQL_MOD: &str = r#"mod module;

pub use module::AuthzGraphqlModule;
"#;

/// The controllers' own chain (`AuthnGuard`, then `AuthzGuard`) at the GraphQL
/// request, joining the app's global pool: the claims and the ability it
/// attaches are ambient for every operation — so one policy answers on both
/// transports, and a resolver reads the claims as `Ambient<Claims>`.
pub(crate) const AUTHZ_GRAPHQL_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::graphql::GraphqlView;
use nest_rs::http::EndpointGuards;

use crate::authn::AuthnGuard;
use crate::authz::{AuthzGuard, AuthzModule};

#[module(
    imports = [AuthzModule],
    providers = [EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)>],
)]
pub struct AuthzGraphqlModule;
"#;

// authz/mcp/ — the guards at `/mcp`'s request (`nestrs g mcp`); without them, or
// a global pool, `/mcp` is deny-all.

pub(crate) const AUTHZ_MCP_MOD: &str = r#"mod module;

pub use module::AuthzMcpModule;
"#;

/// The controllers' own chain (`AuthnGuard`, then `AuthzGuard`) at the MCP
/// request, joining the app's global pool: the ambient `Ability` it attaches is
/// what each row a tool returns is judged under — so one policy answers on every
/// transport.
pub(crate) const AUTHZ_MCP_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::http::EndpointGuards;
use nest_rs::mcp::McpView;

use crate::authn::AuthnGuard;
use crate::authz::{AuthzGuard, AuthzModule};

#[module(
    imports = [AuthzModule],
    providers = [EndpointGuards<McpView, (AuthnGuard, AuthzGuard)>],
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
# (A config pinned in code stops these files being read for the namespace; the
# boot names each key it then ignores.)
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
