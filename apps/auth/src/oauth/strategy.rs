use std::sync::Arc;

use nestrs_authn::{AuthError, JwtService, OAuth2Client, Outcome, Strategy};
use nestrs_core::injectable;
use nestrs_http::async_trait;
use poem::http::{header, StatusCode};
use poem::{Request, Response};
use serde::Deserialize;
use uuid::Uuid;

use identity::Role;

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

#[injectable]
pub struct OAuthStrategy {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    oauth: Arc<OAuth2Client>,
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

        let _profile: serde_json::Value = self.oauth.userinfo(&access_token).await?;
        Ok(Outcome::Authenticated(Caller {
            org_id: Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0001),
            roles: vec![Role::User],
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
