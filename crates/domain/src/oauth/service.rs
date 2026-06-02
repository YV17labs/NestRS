use std::sync::Arc;

use nestrs_authn::JwtService;
use nestrs_core::injectable;
use schemars::JsonSchema;
use serde::Serialize;
use uuid::Uuid;

use crate::oauth::error::TokenError;
use crate::oauth::scope::{role_from_db, roles_for_scope};
use crate::oauth::strategy::AuthenticatedClient;
use crate::users::UsersService;
use crate::{Claims, Role};

#[injectable]
pub struct TokenIssuer {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    users: Arc<UsersService>,
}

/// The OAuth2 token response — the signed bearer token and its lifetime.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

impl TokenIssuer {
    /// Construct with already-resolved dependencies (container or tests).
    pub fn new(jwt: Arc<JwtService>, users: Arc<UsersService>) -> Self {
        Self { jwt, users }
    }

    /// Sign an access token. `sub` is set for human principals; machine grants
    /// (`client_credentials`) omit it.
    pub fn issue(
        &self,
        sub: Option<Uuid>,
        org_id: Uuid,
        roles: Vec<Role>,
    ) -> Result<AccessToken, TokenError> {
        let claims = Claims {
            sub,
            org_id,
            roles: roles.clone(),
            exp: self.jwt.expiry(),
        };
        let access_token = self.jwt.sign(&claims).map_err(|e| TokenError::Sign(e.into()))?;
        tracing::info!(
            ?sub,
            %org_id,
            roles = ?claims.roles,
            "issued access token"
        );
        Ok(AccessToken {
            access_token,
            token_type: "Bearer".into(),
            expires_in: self.jwt.ttl_secs(),
        })
    }

    /// Authenticate with email + password and issue a bearer token.
    pub async fn grant_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AccessToken, TokenError> {
        let user = self
            .users
            .authenticate(email, password)
            .await
            .map_err(|_| TokenError::InvalidCredentials)?;
        let roles = vec![role_from_db(&user.role)];
        self.issue(Some(user.id), user.org_id, roles)
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
        self.issue(None, client.org_id, roles)
    }
}
