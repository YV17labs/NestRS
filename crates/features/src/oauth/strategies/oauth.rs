use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authn::{AuthError, OAuth2Client, Outcome, Strategy};
use nest_rs_core::injectable;
use poem::http::{StatusCode, header};
use poem::{Request, Response};
use serde::Deserialize;

use super::super::service::{Caller, OAuthFlow};

pub type OAuthGuard = nest_rs_authn::AuthGuard<OAuthStrategy>;

const TRANSACTION_COOKIE: &str = "oauth_tx";

#[derive(Debug, Default, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[injectable]
pub struct OAuthStrategy {
    #[inject]
    flow: Arc<OAuthFlow>,
}

#[async_trait]
impl Strategy for OAuthStrategy {
    type Principal = Caller;

    async fn authenticate(&self, req: &mut Request) -> Result<Outcome<Caller>, AuthError> {
        let query: CallbackQuery = req.params().unwrap_or_default();

        let Some(code) = query.code else {
            let authorization = self.flow.authorize()?;
            let secure = cookie_secure(req);
            let redirect = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, authorization.url)
                .header(
                    header::SET_COOKIE,
                    format!(
                        "{TRANSACTION_COOKIE}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{secure}",
                        authorization.transaction,
                        OAuth2Client::TRANSACTION_TTL_SECS,
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
        let caller = self
            .flow
            .resolve_caller(&transaction, &state, &code)
            .await?;
        Ok(Outcome::Authenticated(caller))
    }
}

fn cookie_secure(req: &Request) -> &'static str {
    let https = req.uri().scheme_str() == Some("https")
        || req
            .headers()
            .get("X-Forwarded-Proto")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("https"));
    if https { "; Secure" } else { "" }
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
