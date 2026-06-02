use std::sync::Arc;

use nestrs_authn::{basic_credentials, AuthError, JwtService, OAuth2Client, Outcome, Strategy};
use nestrs_core::injectable;
use nestrs_http::async_trait;
use poem::http::{header, StatusCode};
use poem::{Request, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::oauth::config::IssuerConfig;
use crate::users::UsersService;
use crate::Role;

#[derive(Debug, Clone)]
pub struct Caller {
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

const TRANSACTION_COOKIE: &str = "oauth_tx";

#[derive(Debug, Default, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

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

fn role_from_db(role: &str) -> Role {
    match role {
        "admin" => Role::Admin,
        _ => Role::User,
    }
}

#[injectable]
pub struct OAuthStrategy {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    oauth: Arc<OAuth2Client>,
    #[inject]
    users: Arc<UsersService>,
    #[inject]
    config: Arc<IssuerConfig>,
}

#[async_trait]
impl Strategy for OAuthStrategy {
    type Principal = Caller;

    async fn authenticate(&self, req: &mut Request) -> Result<Outcome<Caller>, AuthError> {
        let query: CallbackQuery = req.params().unwrap_or_default();

        let Some(code) = query.code else {
            let authorization = self.oauth.authorize(&self.jwt)?;
            let redirect = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, authorization.url)
                .header(
                    header::SET_COOKIE,
                    format!(
                        "{TRANSACTION_COOKIE}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=600",
                        authorization.transaction
                    ),
                )
                .finish();
            return Ok(Outcome::Challenge(redirect));
        };

        let state = query
            .state
            .ok_or_else(|| AuthError::Failed("OAuth callback missing state".into()))?;
        let transaction = transaction_cookie(req)
            .ok_or_else(|| AuthError::Failed("OAuth transaction cookie missing".into()))?;
        let access_token = self
            .oauth
            .exchange(&self.jwt, &transaction, &state, &code)
            .await?;

        let profile: OAuthProfile = self.oauth.userinfo(&access_token).await?;
        let user = self
            .users
            .find_or_create(&profile.email(), &profile.display_name(), self.config.default_org_id)
            .await
            .map_err(|err| AuthError::Failed(format!("identity resolution failed: {err}")))?;
        Ok(Outcome::Authenticated(Caller {
            org_id: user.org_id,
            roles: vec![role_from_db(&user.role)],
        }))
    }
}

fn transaction_cookie(req: &Request) -> Option<String> {
    let header = req.headers().get(header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|pair| {
        let pair = pair.trim();
        pair.strip_prefix(TRANSACTION_COOKIE)?
            .strip_prefix('=')
            .map(str::to_owned)
    })
}

pub type OAuthGuard = nestrs_authn::AuthGuard<OAuthStrategy>;

#[derive(Debug, Clone)]
pub struct AuthenticatedClient {
    pub org_id: Uuid,
    pub scopes: Vec<String>,
}

#[injectable]
pub struct ClientCredentialsStrategy {
    #[inject]
    config: Arc<IssuerConfig>,
}

#[async_trait]
impl Strategy for ClientCredentialsStrategy {
    type Principal = AuthenticatedClient;

    async fn authenticate(
        &self,
        req: &mut Request,
    ) -> Result<Outcome<AuthenticatedClient>, AuthError> {
        let (client_id, client_secret) =
            basic_credentials(req).ok_or(AuthError::MissingCredentials)?;
        let client = self
            .config
            .clients
            .iter()
            .find(|client| {
                constant_time_eq(client.client_id.as_bytes(), client_id.as_bytes())
                    && constant_time_eq(client.client_secret.as_bytes(), client_secret.as_bytes())
            })
            .ok_or_else(|| AuthError::Failed("invalid client credentials".into()))?;
        Ok(Outcome::Authenticated(AuthenticatedClient {
            org_id: client.org_id,
            scopes: client.scopes.clone(),
        }))
    }
}

pub type ClientAuthGuard = nestrs_authn::AuthGuard<ClientCredentialsStrategy>;

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
