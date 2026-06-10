use std::sync::Arc;

use crate::oauth::{
    AccessToken, AuthenticatedClient, Caller, ClientAuthGuard, LoginInput, OAuthGuard,
    OAuthService, TokenRequest,
};
use nest_rs_authn::OAuth2Client;
use nest_rs_http::{Ctx, Valid, controller, routes};
use nest_rs_throttler::{Throttle, ThrottlerGuard};
use poem::http::{StatusCode, header};
use poem::web::{Form, Json};
use poem::{Request, Response, Result};

/// Name of the short-lived cookie that carries the signed OAuth transaction
/// (CSRF + PKCE state) between `/authorize` and `/callback`.
pub(crate) const TRANSACTION_COOKIE: &str = "oauth_tx";

#[controller(path = "/")]
pub struct OAuthController {
    #[inject]
    svc: Arc<OAuthService>,
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
        Ok(Json(self.svc.grant_client_credentials(
            &grant_type,
            scope.as_deref(),
            &client,
        )?))
    }

    #[get("/authorize")]
    #[public]
    #[api(
        summary = "OAuth2 authorization endpoint — redirects to the provider",
        tags("OAuth2")
    )]
    async fn authorize(&self, req: &Request) -> Result<Response> {
        // Starting the flow is a redirect, not an authentication: build the
        // 302 + transaction cookie here rather than smuggling a response out
        // of a guard. `/callback` is where `OAuthStrategy` authenticates.
        let authorization = self
            .svc
            .authorize()
            .map_err(poem::error::InternalServerError)?;
        let secure = cookie_secure(req);
        Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, authorization.url)
            .header(
                header::SET_COOKIE,
                format!(
                    "{TRANSACTION_COOKIE}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{secure}",
                    authorization.transaction,
                    OAuth2Client::TRANSACTION_TTL_SECS,
                ),
            )
            .finish())
    }

    #[get("/callback")]
    #[public]
    #[use_guards(OAuthGuard)]
    #[api(
        summary = "OAuth2 redirect URI — issues this app's token",
        tags("OAuth2")
    )]
    async fn callback(&self, caller: Ctx<Caller>) -> Result<Json<AccessToken>> {
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
    async fn login(&self, body: Valid<Json<LoginInput>>) -> Result<Json<AccessToken>> {
        let input = body.into_inner();
        Ok(Json(
            self.svc
                .grant_password(&input.email, &input.password)
                .await?,
        ))
    }
}

/// Append `; Secure` only when the request arrived over HTTPS, so the cookie
/// survives a plaintext local dev round-trip but is hardened in production.
fn cookie_secure(req: &Request) -> &'static str {
    let https = req.uri().scheme_str() == Some("https")
        || req
            .headers()
            .get("X-Forwarded-Proto")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("https"));
    if https { "; Secure" } else { "" }
}
