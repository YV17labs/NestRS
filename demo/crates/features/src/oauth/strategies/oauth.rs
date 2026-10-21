use std::sync::Arc;

use async_trait::async_trait;
use nest_rs::authn::{AuthError, Strategy};
use nest_rs::core::injectable;
use nest_rs::http::{Body, FromRequest, Query, Request, RequestBody, header};
use nest_rs::oauth::client::OAuthClient;
use serde::Deserialize;

use super::super::service::{Caller, OAuthService};

pub type OAuthGuard = nest_rs::authn::AuthnGuard<OAuthStrategy>;

#[derive(Deserialize)]
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
            .path_param("provider")
            .ok_or_else(|| AuthError::failed("OAuth callback missing provider"))?;
        let Ok(Query(query)) =
            Query::<CallbackQuery>::from_request(req, &mut RequestBody::new(Body::empty())).await
        else {
            return Err(AuthError::failed("OAuth callback query does not decode"));
        };
        let code = query
            .code
            .ok_or_else(|| AuthError::failed("OAuth callback missing code"))?;
        let state = query
            .state
            .ok_or_else(|| AuthError::failed("OAuth callback missing state"))?;
        let transaction = OAuthClient::transaction_from(
            req.headers()
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|field| field.to_str().ok()),
        )
        .ok_or_else(|| AuthError::failed("OAuth transaction cookie missing"))?;
        self.svc
            .resolve_caller(provider, transaction, &state, &code)
            .await
    }
}
