//! RFC 6749 §4.4.2 and §5.1 on the wire.

use nest_rs_core::{Discovery, module};
use nest_rs_http::{
    HttpConfig, HttpControllerMeta, HttpModule, IntoResponse, StatusCode, controller, routes,
};
use nest_rs_oauth_server::{AccessTokenRequest, AccessTokenResponse};
use nest_rs_testing::{TestApp, TestResponse};

/// The one encoding RFC 6749 §4.4.2 gives a token request.
const FORM: &str = "application/x-www-form-urlencoded";

/// The JSON `response` answers: its one serialization.
async fn wire(response: AccessTokenResponse) -> serde_json::Value {
    let body = response
        .into_response()
        .into_body()
        .into_bytes()
        .await
        .expect("a body");
    serde_json::from_slice(&body).expect("JSON")
}

#[tokio::test]
async fn the_response_carries_exactly_the_members_5_1_marks_required() {
    let body = wire(AccessTokenResponse::bearer("t", 3600)).await;

    assert_eq!(
        body,
        serde_json::json!({ "access_token": "t", "token_type": "Bearer", "expires_in": 3600 })
    );
}

/// §5.1 and §3.3: an issuer that narrows or defaults the grant says so.
#[tokio::test]
async fn a_granted_scope_is_written_as_the_scope_member() {
    let body = wire(AccessTokenResponse::bearer("t", 3600).with_scope(["posts:read"])).await;

    assert_eq!(body["scope"], "posts:read");
}

#[tokio::test]
async fn several_granted_scopes_are_written_space_delimited() {
    let body =
        wire(AccessTokenResponse::bearer("t", 3600).with_scope(["posts:read", "posts:write"]))
            .await;

    assert_eq!(body["scope"], "posts:read posts:write");
}

/// `#[api(response = …)]` documents the body a bare return no longer spells.
#[controller(path = "/")]
struct IssuerController;

#[routes]
impl IssuerController {
    #[post("/token")]
    #[public]
    #[api(response = AccessTokenResponse)]
    async fn token(&self) -> nest_rs_http::Result<AccessTokenResponse> {
        Ok(AccessTokenResponse::bearer("t", 3600).with_scope(["posts:read"]))
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [IssuerController])]
struct IssuerApp;

/// RFC 6749 §5.1's `no-store` and `no-cache` reach the client through the edge.
#[tokio::test]
async fn a_token_route_answers_no_store_through_the_transport() {
    let app = TestApp::for_module::<IssuerApp>()
        .await
        .expect("the issuer app boots");

    let response = app.http().post("/token").send().await;
    response.assert_status_is_ok();
    response.assert_header("cache-control", "no-store");
    response.assert_header("pragma", "no-cache");
    response
        .assert_json(serde_json::json!({
            "access_token": "t",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "posts:read",
        }))
        .await;
}

#[tokio::test]
async fn the_route_documents_the_access_token_response_it_returns() {
    let app = TestApp::for_module::<IssuerApp>()
        .await
        .expect("the issuer app boots");

    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let route = controllers
        .iter()
        .flat_map(|controller| controller.meta.routes().iter())
        .find(|route| route.handler() == "token")
        .expect("the token route is discovered");
    let schema_of = route.response().expect("a documented response");

    let mut generator = schemars::SchemaGenerator::default();
    let documented = schema_of(&mut generator);
    assert_eq!(
        documented.get("$ref").and_then(serde_json::Value::as_str),
        Some("#/$defs/AccessTokenResponse"),
    );
    let definition = generator
        .definitions()
        .get("AccessTokenResponse")
        .expect("the response's schema is a named component");
    let mut members: Vec<&str> = definition["properties"]
        .as_object()
        .expect("an object schema")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        ["access_token", "expires_in", "scope", "token_type"]
    );
}

/// The token request as an endpoint reads it: §4.4.2's parameters, in the
/// encoding it fixes.
#[controller(path = "/")]
struct RequestController;

#[routes]
impl RequestController {
    #[post("/token")]
    #[public]
    async fn token(&self, request: AccessTokenRequest) -> nest_rs_http::Result<String> {
        Ok(format!("{} {:?}", request.grant_type, request.scope))
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [RequestController])]
struct RequestApp;

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        max_body_bytes: Some(32),
        ..HttpConfig::default()
    })],
    providers = [RequestController],
)]
struct CappedRequestApp;

async fn token_request(app: &TestApp, content_type: &str, body: &str) -> TestResponse {
    app.http()
        .post("/token")
        .content_type(content_type)
        .body(body.to_owned())
        .send()
        .await
}

/// §3.3: `scope` is OPTIONAL, so a request omitting it reaches the issuer.
#[tokio::test]
async fn a_request_omitting_scope_is_read_because_3_3_makes_it_optional() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    let response = token_request(&app, FORM, "grant_type=client_credentials").await;

    response.assert_status_is_ok();
    response.assert_text("client_credentials None").await;
}

/// §5.2 answers an unknown grant with `unsupported_grant_type`, which only the
/// issuer can say: the request reads any grant.
#[tokio::test]
async fn an_unknown_grant_reaches_the_issuer_rather_than_failing_to_decode() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    let response = token_request(
        &app,
        FORM,
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
    )
    .await;

    response.assert_status_is_ok();
    response
        .assert_text("urn:ietf:params:oauth:grant-type:device_code None")
        .await;
}

/// §5.2's `invalid_request`, with §5.1's directives, saying where the request
/// failed.
async fn assert_invalid_request(response: TestResponse, said: &str) {
    response.assert_status(StatusCode::BAD_REQUEST);
    response.assert_header("content-type", "application/json");
    response.assert_header("cache-control", "no-store");
    response.assert_header("pragma", "no-cache");
    assert!(
        response.0.headers().get("www-authenticate").is_none(),
        "{:?}",
        response.0.headers()
    );
    response
        .assert_json(serde_json::json!({
            "error": "invalid_request",
            "error_description": said,
        }))
        .await;
}

/// A token request missing `grant_type`, or sending it with no value (§3.2:
/// "treated as if they were omitted"), reads the same refusal.
#[tokio::test]
async fn a_missing_or_empty_grant_type_is_invalid_request_naming_the_parameter() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    for body in ["scope=posts%3Aread", "grant_type=&scope=posts%3Aread"] {
        assert_invalid_request(
            token_request(&app, FORM, body).await,
            "form field `grant_type`: missing",
        )
        .await;
    }
}

/// §3.2: "Request and response parameters MUST NOT be included more than
/// once" — said by the parameter, never by the value it held.
#[tokio::test]
async fn a_repeated_parameter_is_invalid_request_naming_it_and_never_its_value() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    let response = token_request(
        &app,
        FORM,
        "grant_type=client_credentials&grant_type=sk_live_51",
    )
    .await;

    assert_invalid_request(response, "form field `grant_type`: duplicate").await;
}

/// §4.4.2 fixes the encoding: a JSON body is a malformed token request,
/// answered by the type the endpoint reads, never the one sent.
#[tokio::test]
async fn a_body_that_is_not_a_form_is_invalid_request_naming_only_the_form_type() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    let response = token_request(
        &app,
        "application/json; secret=sk_live_51",
        r#"{"grant_type":"client_credentials"}"#,
    )
    .await;

    assert_invalid_request(
        response,
        "the request's content type is not `application/x-www-form-urlencoded`",
    )
    .await;
}

/// §5.2: a request that "utilizes more than one mechanism for authenticating
/// the client" — the `Authorization` header and a credential in the body.
#[tokio::test]
async fn a_credential_in_the_body_beside_the_header_is_invalid_request() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    for credential in [
        "client_secret=sk_live_51",
        "client_assertion=sk_live_51",
        "client_assertion_type=urn%3Aietf%3Aparams%3Aoauth%3Aclient-assertion-type%3Ajwt-bearer",
    ] {
        let response = app
            .http()
            .post("/token")
            .header("authorization", "Basic Y2k6czNjcmV0")
            .content_type(FORM)
            .body(format!("grant_type=client_credentials&{credential}"))
            .send()
            .await;
        assert_invalid_request(
            response,
            "the client authenticates by more than one mechanism: the `Authorization` header \
             and the body",
        )
        .await;
    }
}

/// §3.2: an empty parameter is an omitted one — `scope=` asks for no scope,
/// `client_secret=` is no second mechanism.
#[tokio::test]
async fn an_empty_parameter_is_read_as_omitted() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    let response = app
        .http()
        .post("/token")
        .header("authorization", "Basic Y2k6czNjcmV0")
        .content_type(FORM)
        .body("grant_type=client_credentials&scope=&client_secret=")
        .send()
        .await;

    response.assert_status_is_ok();
    assert_eq!(
        response.0.into_body().into_string().await.expect("a body"),
        "client_credentials None"
    );
}

/// What the transport refuses before any parameter is read keeps its answer.
#[tokio::test]
async fn a_body_past_the_cap_stays_the_transports_413() {
    let app = TestApp::for_module::<CappedRequestApp>()
        .await
        .expect("boots");

    let response = token_request(
        &app,
        FORM,
        "grant_type=client_credentials&scope=posts%3Aread+posts%3Awrite",
    )
    .await;

    response.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    response.assert_header("content-type", "application/problem+json");
}

/// The extractor documents its body as a `Form<…>` spelling would.
#[tokio::test]
async fn the_route_documents_the_form_body_the_request_reads() {
    let app = TestApp::for_module::<RequestApp>().await.expect("boots");

    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let body = controllers
        .iter()
        .flat_map(|controller| controller.meta.routes().iter())
        .find(|route| route.handler() == "token")
        .and_then(|route| route.request_body())
        .expect("the token route documents its body");

    assert_eq!(body.media_type(), FORM);
    let schema_of = body.schema().expect("a named type");
    let mut generator = schemars::SchemaGenerator::default();
    assert_eq!(
        schema_of(&mut generator)
            .get("$ref")
            .and_then(serde_json::Value::as_str),
        Some("#/$defs/AccessTokenRequest"),
    );
}
