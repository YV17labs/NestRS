use std::sync::Arc;

use crate::oauth::{
    AuthenticatedClient, Caller, ClientAuthnGuard, LoginDto, OAuthGuard, OAuthService,
};
use nest_rs::http::{
    Ctx, Form, HeaderValue, HttpError, Json, Opaque, Path, Piped, Response, Result, StatusCode,
    Valid, controller, header, routes,
};
use nest_rs::oauth::server::{AccessTokenRequest, AccessTokenResponse};
use nest_rs::pipes::Lowercase;
use nest_rs::throttler::{Throttle, ThrottlerGuard};

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
    #[api(
        summary = "OAuth2 token endpoint (client_credentials)",
        tags("OAuth2"),
        response = AccessTokenResponse
    )]
    async fn token(
        &self,
        client: Ctx<AuthenticatedClient>,
        body: Form<AccessTokenRequest>,
    ) -> Result<AccessTokenResponse> {
        let AccessTokenRequest { grant_type, scope } = body.0;
        Ok(self
            .svc
            .grant_client_credentials(&grant_type, scope.as_deref(), &client)?)
    }

    #[get("/social/{provider}/authorize")]
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
            .ok_or_else(|| HttpError::from_status(StatusCode::NOT_FOUND))?
            .opaque()?;
        let set_cookie = HeaderValue::try_from(authorization.set_cookie()).opaque()?;
        let location = HeaderValue::try_from(authorization.url).opaque()?;
        Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(header::SET_COOKIE, set_cookie)
            .header(header::LOCATION, location)
            .finish())
    }

    #[get("/social/{provider}/callback")]
    #[use_guards(ThrottlerGuard, OAuthGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Social login redirect URI — issues this app's token",
        tags("OAuth2"),
        response = AccessTokenResponse
    )]
    async fn social_callback(&self, caller: Ctx<Caller>) -> Result<AccessTokenResponse> {
        Ok(self
            .svc
            .issue(Some(caller.user_id), caller.org_id, caller.roles.clone())?)
    }

    #[post("/login")]
    #[public]
    #[use_guards(ThrottlerGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Sign in with email and password",
        tags("Auth"),
        response = AccessTokenResponse
    )]
    async fn login(&self, body: Valid<Json<LoginDto>>) -> Result<AccessTokenResponse> {
        let input = body.into_inner();
        Ok(self
            .svc
            .grant_password(&input.email, &input.password)
            .await?)
    }
}
