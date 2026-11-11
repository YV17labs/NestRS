//! Covers `src/redirect.rs` and `SocialRegistry::authorize` — the
//! authorization request (RFC 6749 §4.1.1) answered as a `302` to the
//! provider, setting the transaction its callback reads, never cached; a
//! provider the registry does not hold answers `404`.

use std::sync::Arc;

use nest_rs_authn::{AuthnConfig, AuthnModule, JwtService};
use nest_rs_core::{Discovery, module};
use nest_rs_http::{
    HttpControllerMeta, HttpModule, Opaque, Path, Result, StatusCode, controller, header, routes,
};
use nest_rs_oauth_client::OAuthClient;
use nest_rs_social::{SocialModule, SocialRedirect, SocialRegistry};
use nest_rs_testing::TestApp;

use crate::{CODE, KEY, MCP, SECRET, StubSocialModule};

#[controller(path = "/social")]
struct AuthorizeController {
    #[inject]
    providers: Arc<SocialRegistry>,
    #[inject]
    jwt: Arc<JwtService>,
}

#[routes]
impl AuthorizeController {
    #[get("/{provider}/authorize")]
    #[public]
    async fn authorize(&self, Path(provider): Path<String>) -> Result<SocialRedirect> {
        self.providers
            .authorize(&provider, &self.jwt, Some(MCP))
            .opaque()
    }
}

#[module(
    imports = [
        HttpModule,
        AuthnModule::for_root(AuthnConfig {
            secret: Some(SECRET.into()),
            issuer: Some(crate::APP.into()),
            audience: Some(crate::APP.into()),
            ..AuthnConfig::default()
        }),
        SocialModule,
        StubSocialModule,
    ],
    providers = [AuthorizeController],
)]
struct AuthorizeApp;

#[tokio::test]
async fn a_registered_provider_is_redirected_to_with_the_transaction_its_callback_reads() {
    let app = TestApp::for_module::<AuthorizeApp>().await.expect("boots");

    let response = app
        .http()
        .get(format!("/social/{KEY}/authorize"))
        .send()
        .await;

    response.assert_status(StatusCode::FOUND);
    let headers = response.0.headers();
    let location = headers[header::LOCATION].to_str().expect("ASCII");
    assert!(
        location.starts_with("https://provider.example/authorize?"),
        "{location}"
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let set_cookie = headers[header::SET_COOKIE].to_str().expect("ASCII");
    assert!(
        set_cookie.starts_with(&format!("{}=", OAuthClient::TRANSACTION_COOKIE))
            && set_cookie.ends_with("; HttpOnly; Secure; SameSite=Lax"),
        "{set_cookie}"
    );

    let state = location
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("state="))
        .expect("the redirect carries its state");
    let cookie = set_cookie.split(';').next().expect("the cookie's pair");
    let transaction =
        OAuthClient::transaction_from([cookie]).expect("the callback reads the transaction back");
    let jwt = app
        .container()
        .get::<JwtService>()
        .expect("AuthnModule provides the JwtService");
    let matched = OAuthClient::callback(
        &jwt,
        KEY,
        transaction,
        &format!("code={CODE}&state={state}"),
    )
    .expect("the callback matches the transaction the redirect set");
    assert_eq!(matched.resource(), Some(MCP));
}

#[tokio::test]
async fn a_provider_the_registry_does_not_hold_is_not_found() {
    let app = TestApp::for_module::<AuthorizeApp>().await.expect("boots");

    let response = app.http().get("/social/github/authorize").send().await;

    response.assert_status(StatusCode::NOT_FOUND);
    response.assert_header_is_not_exist(header::LOCATION);
    response.assert_header_is_not_exist(header::SET_COOKIE);
    response.assert_content_type("application/problem+json");
    response
        .assert_json(serde_json::json!({
            "type": "about:blank",
            "title": "Not Found",
            "status": 404,
            "code": "NOT_FOUND",
        }))
        .await;
}

#[tokio::test]
async fn the_route_documents_the_redirect_by_its_return_type() {
    let app = TestApp::for_module::<AuthorizeApp>().await.expect("boots");

    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let route = controllers
        .iter()
        .flat_map(|controller| controller.meta.routes().iter())
        .find(|route| route.handler() == "authorize")
        .expect("the authorize route is discovered");
    assert_eq!(route.success_status(), Some(302));
    assert!(route.sets_location());
}
