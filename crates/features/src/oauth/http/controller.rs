use std::sync::Arc;

use crate::oauth::core::{
    AccessToken, AuthenticatedClient, Caller, ClientAuthGuard, LoginInput, OAuthGuard, TokenIssuer,
};
use nestrs_http::{Ctx, Valid, controller, routes};
use nestrs_throttler::{Throttle, ThrottlerGuard};
use poem::Result;
use poem::web::{Form, Json};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[controller(path = "/")]
pub struct OAuthController {
    #[inject]
    issuer: Arc<TokenIssuer>,
}

#[routes]
impl OAuthController {
    #[post("/token")]
    #[use_guards(ThrottlerGuard, ClientAuthGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(summary = "OAuth2 token endpoint (client_credentials)", tags("OAuth2"))]
    async fn token(
        &self,
        client: Ctx<AuthenticatedClient>,
        body: Form<TokenRequest>,
    ) -> Result<Json<AccessToken>> {
        let TokenRequest { grant_type, scope } = body.0;
        Ok(Json(self.issuer.grant_client_credentials(
            &grant_type,
            scope.as_deref(),
            &client,
        )?))
    }

    #[get("/authorize")]
    #[use_guards(OAuthGuard)]
    #[api(
        summary = "OAuth2 authorization endpoint — redirects to the provider",
        tags("OAuth2")
    )]
    async fn authorize(&self) {}

    #[get("/callback")]
    #[use_guards(OAuthGuard)]
    #[api(
        summary = "OAuth2 redirect URI — issues this app's token",
        tags("OAuth2")
    )]
    async fn callback(&self, caller: Ctx<Caller>) -> Result<Json<AccessToken>> {
        Ok(Json(self.issuer.issue(
            Some(caller.user_id),
            caller.org_id,
            caller.roles.clone(),
        )?))
    }

    #[post("/login")]
    #[use_guards(ThrottlerGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(summary = "Sign in with email and password", tags("Auth"))]
    async fn login(&self, body: Valid<Json<LoginInput>>) -> Result<Json<AccessToken>> {
        let input = body.into_inner();
        Ok(Json(
            self.issuer
                .grant_password(&input.email, &input.password)
                .await?,
        ))
    }
}
