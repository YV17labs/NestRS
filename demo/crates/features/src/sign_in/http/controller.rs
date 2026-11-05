use std::sync::Arc;

use nest_rs::core::Ambient;
use nest_rs::http::{Json, Path, Query, Result, controller, routes};
use nest_rs::oauth::server::{AccessTokenRequest, AccessTokenResponse, ClientGuard, TokenError};
use nest_rs::pipes::{Lowercase, Piped};
use nest_rs::social::SocialRedirect;
use nest_rs::throttler::{Throttle, ThrottlerGuard};

use super::guard::SocialGuard;
use crate::sign_in::{
    AuthenticatedClient, AuthorizeDto, Caller, ClientPayload, LoginDto, SignInService,
};

#[controller(path = "/")]
pub struct SignInController {
    #[inject]
    svc: Arc<SignInService>,
}

#[routes]
impl SignInController {
    #[post("/token")]
    #[use_guards(ThrottlerGuard, ClientGuard<ClientPayload>)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "OAuth2 token endpoint (client_credentials)",
        tags("OAuth2"),
        error(500 = TokenError)
    )]
    async fn token(
        &self,
        client: Ambient<AuthenticatedClient>,
        request: AccessTokenRequest,
    ) -> Result<AccessTokenResponse> {
        Ok(self.svc.grant_client_credentials(&request, &client)?)
    }

    #[get("/social/{provider}/authorize")]
    #[public]
    #[use_guards(ThrottlerGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Social login — redirects to the named provider",
        tags("OAuth2")
    )]
    async fn social_authorize(
        &self,
        Piped(provider): Piped<Lowercase, Path<String>>,
        query: Query<AuthorizeDto>,
    ) -> Result<SocialRedirect> {
        Ok(self.svc.authorize(&provider, query.0.resource.as_deref())?)
    }

    #[get("/social/{provider}/callback")]
    #[use_guards(ThrottlerGuard, SocialGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Social login redirect URI — issues this app's token",
        tags("OAuth2")
    )]
    async fn social_callback(&self, caller: Ambient<Caller>) -> Result<AccessTokenResponse> {
        Ok(self.svc.issue(
            caller.resource.as_deref(),
            caller.user_id,
            caller.org_id,
            caller.roles.clone(),
        )?)
    }

    #[post("/login")]
    #[public]
    #[use_guards(ThrottlerGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(summary = "Sign in with email and password", tags("Auth"))]
    async fn login(&self, Json(input): Json<LoginDto>) -> Result<AccessTokenResponse> {
        Ok(self
            .svc
            .grant_password(&input.email, &input.password, input.resource.as_deref())
            .await?)
    }
}
