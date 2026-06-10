use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authn::{AuthError, Strategy};
use nest_rs_core::injectable;
use poem::Request;
use poem::http::header;
use serde::Deserialize;

use super::super::http::TRANSACTION_COOKIE;
use super::super::service::{Caller, OAuthService};

pub type OAuthGuard = nest_rs_authn::AuthGuard<OAuthStrategy>;

#[derive(Debug, Default, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[injectable]
pub struct OAuthStrategy {
    #[inject]
    svc: Arc<OAuthService>,
}

/// Resolves the principal from the provider's redirect back to `/callback`.
/// The `/authorize` leg that *starts* the flow is a plain public handler
/// (`OAuthController::authorize`) — it issues a redirect, which is a handler
/// response, not an authentication outcome.
#[async_trait]
impl Strategy for OAuthStrategy {
    type Principal = Caller;

    async fn authenticate(&self, req: &mut Request) -> Result<Caller, AuthError> {
        let query: CallbackQuery = req.params().unwrap_or_default();
        let code = query
            .code
            .ok_or_else(|| AuthError::Failed("OAuth callback missing code".into()))?;
        let state = query
            .state
            .ok_or_else(|| AuthError::Failed("OAuth callback missing state".into()))?;
        let transaction = transaction_cookie(req)
            .ok_or_else(|| AuthError::Failed("OAuth transaction cookie missing".into()))?;
        self.svc.resolve_caller(&transaction, &state, &code).await
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
