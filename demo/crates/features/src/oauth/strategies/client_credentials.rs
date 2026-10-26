use std::sync::Arc;

use async_trait::async_trait;
use nest_rs::authn::{AuthError, Strategy, basic_credentials};
use nest_rs::core::injectable;
use nest_rs::http::Request;
use nest_rs::oauth::server::ClientAuthenticationGuard;

use super::super::service::{AuthenticatedClient, OAuthService};

pub type ClientAuthnGuard = ClientAuthenticationGuard<ClientCredentialsStrategy>;

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
            basic_credentials(req.headers()).ok_or(AuthError::MissingCredentials)?;
        let client = self.svc.authenticate_client(&client_id, &client_secret)?;
        Ok(client)
    }
}
