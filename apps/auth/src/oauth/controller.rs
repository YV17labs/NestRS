use std::sync::Arc;

use nestrs_http::{controller, routes, Ctx};
use nestrs_throttler::{Throttle, ThrottlerGuard};
use poem::web::{Form, Json};
use poem::Result;
use serde::Deserialize;

use domain::oauth::{
    AccessToken, AuthenticatedClient, Caller, ClientAuthGuard, OAuthGuard, TokenIssuer,
};

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
        Ok(Json(self.issuer.issue(caller.org_id, caller.roles.clone())?))
    }
}
