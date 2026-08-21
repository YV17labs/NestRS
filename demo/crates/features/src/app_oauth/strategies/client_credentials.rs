use std::sync::Arc;

use async_trait::async_trait;
use nest_rs::authn::{AuthError, Strategy, basic_credentials};
use nest_rs::core::injectable;
use poem::Request;

use super::super::service::{AppOAuthService, AuthenticatedClient};

pub type ClientAuthnGuard = nest_rs::authn::AuthnGuard<ClientCredentialsStrategy>;

#[injectable]
pub struct ClientCredentialsStrategy {
    #[inject]
    svc: Arc<AppOAuthService>,
}

#[async_trait]
impl Strategy for ClientCredentialsStrategy {
    type Principal = AuthenticatedClient;

    async fn authenticate(&self, req: &mut Request) -> Result<AuthenticatedClient, AuthError> {
        let (client_id, client_secret) =
            basic_credentials(req).ok_or(AuthError::MissingCredentials)?;
        let client = self.svc.authenticate_client(&client_id, &client_secret)?;
        Ok(client)
    }
}
