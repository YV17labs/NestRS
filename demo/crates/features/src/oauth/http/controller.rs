use std::sync::Arc;

use crate::oauth::{
    AccessTokenDto, AuthenticatedClient, Caller, ClientAuthnGuard, LoginDto, OAuthGuard,
    OAuthService, TokenRequestDto,
};
use nest_rs_authn::OAuth2Client;
use nest_rs_http::{Ctx, Piped, Valid, controller, routes};
use nest_rs_pipes::Lowercase;
use nest_rs_throttler::{Throttle, ThrottlerGuard};
use poem::http::{StatusCode, header};
use poem::web::{Form, Json, Path};
use poem::{Response, Result};

pub(crate) const TRANSACTION_COOKIE: &str = "oauth_tx";

#[controller(path = "/")]
pub struct OAuthController {
    #[inject]
    svc: Arc<OAuthService>,
}

#[routes]
impl OAuthController {
    #[post("/token")]
    #[use_guards(ThrottlerGuard, ClientAuthnGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(summary = "OAuth2 token endpoint (client_credentials)", tags("OAuth2"))]
    async fn token(
        &self,
        client: Ctx<AuthenticatedClient>,
        body: Form<TokenRequestDto>,
    ) -> Result<Json<AccessTokenDto>> {
        let TokenRequestDto { grant_type, scope } = body.0;
        Ok(Json(self.svc.grant_client_credentials(
            &grant_type,
            scope.as_deref(),
            &client,
        )?))
    }

    #[get("/social/:provider/authorize")]
    #[public]
    #[use_guards(ThrottlerGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Social login — redirects to the named provider",
        tags("OAuth2")
    )]
    async fn social_authorize(&self, provider: Piped<Lowercase, Path<String>>) -> Result<Response> {
        let authorization = self
            .svc
            .authorize(&provider.into_inner())
            .ok_or_else(|| poem::Error::from_status(StatusCode::NOT_FOUND))?
            .map_err(poem::error::InternalServerError)?;
        Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, authorization.url)
            .header(
                header::SET_COOKIE,
                format!(
                    "{TRANSACTION_COOKIE}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{COOKIE_SECURE}",
                    authorization.transaction,
                    OAuth2Client::TRANSACTION_TTL_SECS,
                ),
            )
            .finish())
    }

    #[get("/social/:provider/callback")]
    #[use_guards(ThrottlerGuard, OAuthGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Social login redirect URI — issues this app's token",
        tags("OAuth2")
    )]
    async fn social_callback(&self, caller: Ctx<Caller>) -> Result<Json<AccessTokenDto>> {
        Ok(Json(self.svc.issue(
            Some(caller.user_id),
            caller.org_id,
            caller.roles.clone(),
        )?))
    }

    #[post("/login")]
    #[public]
    #[use_guards(ThrottlerGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(summary = "Sign in with email and password", tags("Auth"))]
    async fn login(&self, body: Valid<Json<LoginDto>>) -> Result<Json<AccessTokenDto>> {
        let input = body.into_inner();
        Ok(Json(
            self.svc
                .grant_password(&input.email, &input.password)
                .await?,
        ))
    }
}

const COOKIE_SECURE: &str = "; Secure";
