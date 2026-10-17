use std::sync::Arc;

use async_trait::async_trait;
use nest_rs::authn::{AuthError, Strategy};
use nest_rs::core::injectable;
use nest_rs::oauth::client::OAuthClient;
use poem::Request;
use poem::http::header;
use serde::Deserialize;

use super::super::service::{Caller, OAuthService};

pub type OAuthGuard = nest_rs::authn::AuthnGuard<OAuthStrategy>;

#[derive(Default, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[injectable]
pub struct OAuthStrategy {
    #[inject]
    svc: Arc<OAuthService>,
}

#[async_trait]
impl Strategy for OAuthStrategy {
    type Principal = Caller;

    async fn authenticate(&self, req: &mut Request) -> Result<Caller, AuthError> {
        let provider = req
            .raw_path_param("provider")
            .ok_or_else(|| AuthError::Failed("OAuth callback missing provider".into()))?
            .to_owned();
        let query: CallbackQuery = req.params().unwrap_or_default();
        let code = query
            .code
            .ok_or_else(|| AuthError::Failed("OAuth callback missing code".into()))?;
        let state = query
            .state
            .ok_or_else(|| AuthError::Failed("OAuth callback missing state".into()))?;
        let transaction = OAuthClient::transaction_from(
            req.headers()
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|field| field.to_str().ok()),
        )
        .ok_or_else(|| AuthError::Failed("OAuth transaction cookie missing".into()))?;
        self.svc
            .resolve_caller(&provider, transaction, &state, &code)
            .await
    }
}
