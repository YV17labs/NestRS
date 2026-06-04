use std::sync::Arc;

use nestrs_authn::{AuthError, Authorization, JwtService, OAuth2Client};
use nestrs_core::injectable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::IssuerConfig;
use super::error::TokenError;
use super::scope::{role_from_db, roles_for_scope};
use crate::users::UsersService;
use crate::{Claims, Role};

#[derive(Debug, Clone)]
pub struct Caller {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedClient {
    pub org_id: Uuid,
    pub scopes: Vec<String>,
}

/// `id` is the only field providers always return; the rest falls back to
/// derived values so a missing email or display name does not block account
/// creation.
#[derive(Debug, Clone, Deserialize)]
struct OAuthProfile {
    id: i64,
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl OAuthProfile {
    fn email(&self) -> String {
        self.email
            .clone()
            .filter(|email| !email.is_empty())
            .unwrap_or_else(|| format!("{}@users.noreply.github.com", self.handle()))
    }

    fn handle(&self) -> String {
        self.login.clone().unwrap_or_else(|| self.id.to_string())
    }

    fn display_name(&self) -> String {
        self.name
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| self.login.clone())
            .unwrap_or_else(|| format!("user-{}", self.id))
    }
}

#[injectable]
pub struct TokenIssuer {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    users: Arc<UsersService>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

impl TokenIssuer {
    pub fn new(jwt: Arc<JwtService>, users: Arc<UsersService>) -> Self {
        Self { jwt, users }
    }

    /// `sub` is set for human principals; machine grants
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
        let access_token = self
            .jwt
            .sign(&claims)
            .map_err(|e| TokenError::Sign(e.into()))?;
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

#[injectable]
pub struct OAuthFlow {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    oauth: Arc<OAuth2Client>,
    #[inject]
    users: Arc<UsersService>,
    #[inject]
    config: Arc<IssuerConfig>,
}

impl OAuthFlow {
    pub fn new(
        jwt: Arc<JwtService>,
        oauth: Arc<OAuth2Client>,
        users: Arc<UsersService>,
        config: Arc<IssuerConfig>,
    ) -> Self {
        Self {
            jwt,
            oauth,
            users,
            config,
        }
    }

    pub fn authorize(&self) -> Result<Authorization, AuthError> {
        self.oauth.authorize(&self.jwt)
    }

    pub async fn resolve_caller(
        &self,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<Caller, AuthError> {
        let access_token = self
            .oauth
            .exchange(&self.jwt, transaction, state, code)
            .await?;
        let profile: OAuthProfile = self.oauth.userinfo(&access_token).await?;
        let user = self
            .users
            .find_or_create(
                &profile.email(),
                &profile.display_name(),
                self.config.default_org_id,
            )
            .await
            .map_err(|err| AuthError::Failed(format!("identity resolution failed: {err}")))?;
        Ok(Caller {
            user_id: user.id,
            org_id: user.org_id,
            roles: vec![role_from_db(&user.role)],
        })
    }

    /// Constant-time comparison against the configured client registry.
    pub fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<AuthenticatedClient, AuthError> {
        let client = self
            .config
            .clients
            .iter()
            .find(|client| {
                constant_time_eq(client.client_id.as_bytes(), client_id.as_bytes())
                    && constant_time_eq(client.client_secret.as_bytes(), client_secret.as_bytes())
            })
            .ok_or_else(|| AuthError::Failed("invalid client credentials".into()))?;
        Ok(AuthenticatedClient {
            org_id: client.org_id,
            scopes: client.scopes.clone(),
        })
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
