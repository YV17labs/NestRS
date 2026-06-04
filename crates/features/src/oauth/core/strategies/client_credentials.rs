use std::sync::Arc;

use async_trait::async_trait;
use nestrs_authn::{AuthError, Outcome, Strategy, basic_credentials};
use nestrs_core::injectable;
use poem::Request;

use super::super::service::{AuthenticatedClient, OAuthFlow};

pub type ClientAuthGuard = nestrs_authn::AuthGuard<ClientCredentialsStrategy>;

#[injectable]
pub struct ClientCredentialsStrategy {
    #[inject]
    flow: Arc<OAuthFlow>,
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
        let client = self.flow.authenticate_client(&client_id, &client_secret)?;
        Ok(Outcome::Authenticated(client))
    }
}
