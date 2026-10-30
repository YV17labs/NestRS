use std::sync::Arc;

use nest_rs::core::Ctx;
use nest_rs::http::{
    HeaderValue, HttpError, Json, Opaque, Path, Query, Response, Result, StatusCode, controller,
    header, routes,
};
use nest_rs::oauth::server::{AccessTokenRequest, AccessTokenResponse};
use nest_rs::pipes::{Lowercase, Piped};
use nest_rs::throttler::{Throttle, ThrottlerGuard};

use super::guard::{ClientGuard, SocialGuard};
use crate::sign_in::{AuthenticatedClient, AuthorizeDto, Caller, LoginDto, SignInService};

#[controller(path = "/")]
pub struct SignInController {
    #[inject]
    svc: Arc<SignInService>,
}

#[routes]
impl SignInController {
    #[post("/token")]
    #[use_guards(ThrottlerGuard, ClientGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "OAuth2 token endpoint (client_credentials)",
        tags("OAuth2"),
        response = AccessTokenResponse
    )]
    async fn token(
        &self,
        client: Ctx<AuthenticatedClient>,
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
    ) -> Result<Response> {
        let authorization = self
            .svc
            .authorize(&provider, query.0.resource.as_deref())?
            .ok_or_else(|| HttpError::from_status(StatusCode::NOT_FOUND))?;
        let set_cookie = HeaderValue::try_from(authorization.set_cookie()).opaque()?;
        let location = HeaderValue::try_from(authorization.url).opaque()?;
        Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(header::SET_COOKIE, set_cookie)
            .header(header::LOCATION, location)
            .finish())
    }

    #[get("/social/{provider}/callback")]
    #[use_guards(ThrottlerGuard, SocialGuard)]
    #[meta(Throttle::per_minute(10))]
    #[api(
        summary = "Social login redirect URI — issues this app's token",
        tags("OAuth2"),
        response = AccessTokenResponse
    )]
    async fn social_callback(&self, caller: Ctx<Caller>) -> Result<AccessTokenResponse> {
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
    #[api(
        summary = "Sign in with email and password",
        tags("Auth"),
        response = AccessTokenResponse
    )]
    async fn login(&self, Json(input): Json<LoginDto>) -> Result<AccessTokenResponse> {
        Ok(self
            .svc
            .grant_password(&input.email, &input.password, input.resource.as_deref())
            .await?)
    }
}
