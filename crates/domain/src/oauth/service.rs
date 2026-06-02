use std::sync::Arc;

use nestrs_authn::JwtService;
use nestrs_core::injectable;
use poem::error::ResponseError;
use poem::http::StatusCode;
use schemars::JsonSchema;
use serde::Serialize;
use uuid::Uuid;

use crate::oauth::strategy::AuthenticatedClient;
use crate::{Claims, Role};

#[injectable]
pub struct TokenIssuer {
    #[inject]
    jwt: Arc<JwtService>,
}

/// The OAuth2 token response — the signed bearer token and its lifetime.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

impl TokenIssuer {
    /// Sign an access token carrying `roles` for `org_id`.
    pub fn issue(&self, org_id: Uuid, roles: Vec<Role>) -> Result<AccessToken, TokenError> {
        let claims = Claims {
            org_id,
            roles,
            exp: self.jwt.expiry(),
        };
        let access_token = self.jwt.sign(&claims).map_err(|e| TokenError::Sign(e.into()))?;
        tracing::info!(%org_id, roles = ?claims.roles, "issued access token");
        Ok(AccessToken {
            access_token,
            token_type: "Bearer".into(),
            expires_in: self.jwt.ttl_secs(),
        })
    }

    /// The `client_credentials` grant: validate the grant type, resolve the
    /// requested scope to roles within the client's grant, then issue a token.
    pub fn grant_client_credentials(
        &self,
        grant_type: &str,
        scope: Option<&str>,
        client: &AuthenticatedClient,
    ) -> Result<AccessToken, TokenError> {
        if grant_type != "client_credentials" {
            return Err(TokenError::UnsupportedGrant);
        }
        let roles = roles_for_scope(scope, &client.scopes).ok_or(TokenError::InvalidScope)?;
        self.issue(client.org_id, roles)
    }
}

/// Resolve a requested OAuth `scope` to the roles it grants, constrained to the
/// client's `allowed` scopes. Empty or absent grants the client's full set; an
/// unknown scope yields `None` (rejected upstream as `invalid_scope`).
fn roles_for_scope(requested: Option<&str>, allowed: &[String]) -> Option<Vec<Role>> {
    let granted: Vec<&str> = match requested {
        Some(raw) if !raw.trim().is_empty() => {
            let requested: Vec<&str> = raw.split_whitespace().collect();
            if requested
                .iter()
                .any(|s| !allowed.iter().any(|grant| grant == s))
            {
                return None;
            }
            requested
        }
        _ => allowed.iter().map(String::as_str).collect(),
    };
    let roles: Vec<Role> = granted
        .iter()
        .filter_map(|scope| match *scope {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        })
        .collect();
    Some(if roles.is_empty() {
        vec![Role::User]
    } else {
        roles
    })
}

/// A token request that cannot be fulfilled. The `Display` strings are the OAuth2
/// error codes; [`ResponseError`] maps each to its HTTP status.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("unsupported_grant_type")]
    UnsupportedGrant,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("server_error")]
    Sign(#[source] anyhow::Error),
}

impl ResponseError for TokenError {
    fn status(&self) -> StatusCode {
        match self {
            TokenError::Sign(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}
