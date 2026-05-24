//! Authentication: a guard that resolves the calling principal and attaches it
//! as [`AuthUser`] for downstream guards/handlers to read via
//! `nestrs_http::Ctx<AuthUser>`.
//!
//! A deliberately minimal stand-in for real auth (JWT, sessions): the claims a
//! signed token would carry — tenant (`org_id`), roles — are read from request
//! headers instead. It exercises the guard → request-context → authorization
//! path end to end without a token library.

use nestrs_core::injectable;
use nestrs_http::{async_trait, Guard};
use poem::http::StatusCode;
use poem::{Request, Response};
use uuid::Uuid;

/// A role claim carried by the principal. Authorization rules
/// ([`crate::authz::AppAbility`]) map roles to abilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    User,
}

/// The authenticated principal, attached by [`AuthGuard`] and read by the
/// ability guard and handlers via `nestrs_http::Ctx<AuthUser>`.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}

/// Rejects a request without a non-empty `x-api-key` and a valid `x-org-id`
/// (`401`); on success attaches an [`AuthUser`]. Roles come from a comma-list in
/// `x-roles` (default `user`), the user id from `x-user-id` (default a fresh v7).
/// An `#[injectable]` provider, bound to routes with `#[use_guards(AuthGuard)]`.
#[injectable]
#[derive(Default)]
pub struct AuthGuard;

#[async_trait]
impl Guard for AuthGuard {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        // Own every header value before borrowing the request mutably to attach
        // the principal.
        let header = |name: &str| {
            req.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let api_key = header("x-api-key").filter(|k| !k.is_empty());
        let org_id = header("x-org-id").and_then(|s| Uuid::parse_str(&s).ok());
        let roles = header("x-roles").map(|s| parse_roles(&s));

        if api_key.is_none() {
            return Err(unauthorized("missing or empty x-api-key header"));
        }
        let Some(org_id) = org_id else {
            return Err(unauthorized("missing or invalid x-org-id header (expected a UUID)"));
        };

        req.extensions_mut().insert(AuthUser {
            org_id,
            roles: roles.unwrap_or_else(|| vec![Role::User]),
        });
        Ok(())
    }
}

fn parse_roles(raw: &str) -> Vec<Role> {
    let roles: Vec<Role> = raw
        .split(',')
        .filter_map(|token| match token.trim().to_ascii_lowercase().as_str() {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        })
        .collect();
    if roles.is_empty() {
        vec![Role::User]
    } else {
        roles
    }
}

fn unauthorized(message: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(message)
}
