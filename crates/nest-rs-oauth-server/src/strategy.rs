//! [`ClientSecretBasicStrategy`] — RFC 6749 §2.3.1's HTTP Basic client
//! authentication against the registry [`OAuthServerConfig`] holds.

use std::sync::Arc;

use nest_rs_authn::{AuthError, Strategy, basic_credentials};
use nest_rs_core::{async_trait, injectable};
use nest_rs_http::Request;

use crate::config::OAuthServerConfig;
use crate::registry::{AuthenticatedClient, authenticate_against_registry};

/// The [`Strategy`] a token endpoint's
/// [`ClientAuthenticationGuard`](crate::ClientAuthenticationGuard) runs for RFC
/// 7591's `client_secret_basic`: reads the client's id and secret off the
/// `Authorization: Basic` header (§2.3.1) and authenticates them, in constant
/// time, against the clients [`OAuthServerConfig<P>`] registers. A request
/// without the header is [`AuthError::MissingCredentials`]; a pair no client
/// matches is refused without saying which half failed.
///
/// ```
/// use nest_rs_core::{Ambient, module};
/// use nest_rs_http::{controller, routes};
/// use nest_rs_oauth_server::{
///     AccessTokenRequest, AccessTokenResponse, AuthenticatedClient, ClientAuthenticationGuard,
///     ClientSecretBasicStrategy, OAuthServerModule,
/// };
///
/// type ClientGuard = ClientAuthenticationGuard<ClientSecretBasicStrategy<()>>;
///
/// #[controller(path = "/")]
/// struct TokenController;
///
/// #[routes]
/// impl TokenController {
///     #[post("/token")]
///     #[use_guards(ClientGuard)]
///     #[api(response = AccessTokenResponse)]
///     async fn token(
///         &self,
///         client: Ambient<AuthenticatedClient<()>>,
///         request: AccessTokenRequest,
///     ) -> nest_rs_http::Result<AccessTokenResponse> {
///         let scope = client.scope_for(&request.scope)?;
///         let _audience = client.audience_for(&request.resource)?;
///         Ok(AccessTokenResponse::bearer("t", 3600).with_scope(scope))
///     }
/// }
///
/// #[module(
///     imports = [OAuthServerModule::<()>],
///     providers = [ClientSecretBasicStrategy<()>, ClientGuard, TokenController],
/// )]
/// struct IssuerModule;
/// ```
#[injectable]
pub struct ClientSecretBasicStrategy<P: Send + Sync + 'static> {
    #[inject]
    config: Arc<OAuthServerConfig<P>>,
}

impl<P: Send + Sync + 'static> ClientSecretBasicStrategy<P> {
    /// Construct over an already-resolved registry (container or tests).
    pub fn new(config: Arc<OAuthServerConfig<P>>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl<P: Clone + Send + Sync + 'static> Strategy for ClientSecretBasicStrategy<P> {
    type Principal = AuthenticatedClient<P>;

    async fn authenticate(&self, req: &mut Request) -> Result<AuthenticatedClient<P>, AuthError> {
        let (client_id, client_secret) =
            basic_credentials(req.headers()).ok_or(AuthError::MissingCredentials)?;
        authenticate_against_registry(&self.config.clients, &client_id, &client_secret)
    }
}
