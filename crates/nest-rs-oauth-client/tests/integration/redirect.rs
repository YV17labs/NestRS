//! Covers `src/redirect.rs` — the authorization request (RFC 6749 §4.1.1)
//! answered as a `302` to the provider, setting the transaction its callback
//! reads, never cached, and the document that says so.

use std::sync::Arc;

use nest_rs_authn::{AuthnConfig, AuthnModule, JwtService};
use nest_rs_config::Surface;
use nest_rs_core::module;
use nest_rs_http::{HttpModule, Opaque, Result, StatusCode, controller, header, routes};
use nest_rs_oauth_client::{AuthorizationRedirect, OAuthClient, OAuthClientModule, TARGET};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule};
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::Value;
use tracing::Level;

use super::config::valid_config;

const PROVIDER: &str = "acme";
const RESOURCE: &str = "https://api.example";

#[controller(path = "/")]
struct OAuthController {
    #[inject]
    client: Arc<OAuthClient>,
    #[inject]
    jwt: Arc<JwtService>,
}

#[routes]
impl OAuthController {
    #[get("/authorize")]
    #[public]
    async fn authorize(&self) -> Result<AuthorizationRedirect> {
        self.client
            .authorize(&self.jwt, PROVIDER, Some(RESOURCE))
            .opaque()
    }

    /// The one way to the refusal: a provider's override appending to the URL
    /// it was handed.
    #[get("/authorize/appended")]
    #[public]
    async fn appended(&self) -> Result<AuthorizationRedirect> {
        let mut redirect = self
            .client
            .authorize(&self.jwt, PROVIDER, Some(RESOURCE))
            .opaque()?;
        redirect.url.push('\n');
        Ok(redirect)
    }
}

#[module(
    imports = [
        HttpModule,
        AuthnModule::for_root(AuthnConfig {
            secret: Some(crate::SECRET.into()),
            ..AuthnConfig::default()
        }),
        OAuthClientModule::for_root(valid_config()),
        OpenApiModule::for_root(OpenApiConfig {
            enabled: Surface::Open,
            ..OpenApiConfig::default()
        }),
    ],
    providers = [OAuthController],
)]
struct AuthorizeApp;

#[tokio::test]
async fn a_handler_returning_the_redirect_answers_the_302_with_its_transaction_never_cached() {
    let app = TestApp::for_module::<AuthorizeApp>().await.expect("boots");

    let response = app.http().get("/authorize").send().await;

    response.assert_status(StatusCode::FOUND);
    let headers = response.0.headers();
    let location = headers[header::LOCATION].to_str().expect("ASCII");
    assert!(
        location.starts_with(&format!("{}?", valid_config().auth_url)),
        "{location}"
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let set_cookie = headers[header::SET_COOKIE].to_str().expect("ASCII");
    let (cookie, attributes) = set_cookie
        .split_once("; ")
        .expect("the cookie's attributes");
    assert_eq!(
        attributes,
        format!(
            "Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Lax",
            OAuthClient::TRANSACTION_TTL_SECS,
        ),
    );
    let (name, _) = cookie.split_once('=').expect("the cookie's name");
    assert_eq!(name, OAuthClient::TRANSACTION_COOKIE);

    let state = location
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("state="))
        .expect("the redirect carries its state");
    let transaction =
        OAuthClient::transaction_from([cookie]).expect("the callback reads the transaction back");
    let jwt = app
        .container()
        .get::<JwtService>()
        .expect("AuthnModule provides the JwtService");
    let matched = OAuthClient::callback(
        &jwt,
        PROVIDER,
        transaction,
        &format!("code=the-code&state={state}"),
    )
    .expect("the callback matches the transaction the redirect set");
    assert_eq!(matched.resource(), Some(RESOURCE));
}

#[tokio::test]
async fn a_url_no_header_can_carry_reaches_the_client_opaque_and_the_operator_on_one_line() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<AuthorizeApp>().await.expect("boots");

    let response = app.http().get("/authorize/appended").send().await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.0.headers().get(header::SET_COOKIE).is_none());
    let document: Value = response.json().await.value().deserialize();
    assert_eq!(document["code"], "INTERNAL", "{document}");
    assert!(document.get("detail").is_none(), "{document}");
    logs.expect_one(TARGET, "OAuth redirect rejected");
    let errors = logs.events_at(Level::ERROR);
    assert_eq!(
        errors.len(),
        1,
        "the edge files nothing beside it: {errors:#?}"
    );
}

#[tokio::test]
async fn the_document_names_the_302_and_its_location_by_the_return_type() {
    let app = TestApp::for_module::<AuthorizeApp>().await.expect("boots");

    let response = app.http().get("/api-json").send().await;

    response.assert_status_is_ok();
    let doc: Value = response.json().await.value().deserialize();
    let responses = &doc["paths"]["/authorize"]["get"]["responses"];
    assert!(responses.get("200").is_none(), "{responses}");
    assert_eq!(
        responses["302"]["headers"]["Location"]["schema"]["format"], "uri-reference",
        "{responses}",
    );
}
