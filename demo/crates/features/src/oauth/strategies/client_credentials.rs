use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authn::{AuthError, Strategy, basic_credentials};
use nest_rs_core::injectable;
use poem::Request;

use super::super::service::{AuthenticatedClient, OAuthService};

pub type ClientAuthGuard = nest_rs_authn::AuthGuard<ClientCredentialsStrategy>;

#[injectable]
pub struct ClientCredentialsStrategy {
    #[inject]
    svc: Arc<OAuthService>,
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
