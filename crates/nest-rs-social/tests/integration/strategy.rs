//! Covers `src/strategy.rs` — the redirect URI's RFC 6749 §4.1.2 response, read
//! against the transaction that began the flow, signs the provider's profile in
//! through the product's `SocialSignIn`, carrying the resource the flow asked
//! for; §4.1.2.1's refusal and a response nobody began never reach it.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authn::{
    AuthError, AuthnConfig, AuthnGuard, AuthnModule, JwtService, PrincipalIdentity,
};
use nest_rs_core::{Ambient, AmbientValue, async_trait, injectable, module};
use nest_rs_http::{HttpModule, IntoResponse as _, Json, StatusCode, controller, header, routes};
use nest_rs_social::{SocialModule, SocialProfile, SocialProvider, SocialSignIn, SocialStrategy};
use nest_rs_testing::{TestApp, TestResponse};

use crate::{CODE, KEY, MCP, SECRET, StubSocialModule, stub_provider};

#[derive(Clone)]
struct Member {
    subject: String,
    resource: Option<String>,
}

impl AmbientValue for Member {}

impl PrincipalIdentity for Member {
    fn actor_id(&self) -> Option<String> {
        Some(self.subject.clone())
    }
}

/// The product's sign-in, counting the profiles it was handed.
#[injectable]
#[derive(Default)]
struct MembersSignIn {
    signed_in: AtomicUsize,
}

#[async_trait]
impl SocialSignIn for MembersSignIn {
    type Principal = Member;

    async fn sign_in(
        &self,
        profile: SocialProfile,
        resource: Option<&str>,
    ) -> Result<Member, AuthError> {
        self.signed_in.fetch_add(1, Ordering::SeqCst);
        Ok(Member {
            subject: format!("{}:{}", profile.provider, profile.subject),
            resource: resource.map(str::to_owned),
        })
    }
}

type SocialGuard = AuthnGuard<SocialStrategy<MembersSignIn>>;

#[controller(path = "/social")]
struct CallbackController;

#[routes]
impl CallbackController {
    #[get("/{provider}/callback")]
    #[use_guards(SocialGuard)]
    async fn callback(&self, member: Ambient<Member>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "subject": member.subject,
            "resource": member.resource,
        }))
    }
}

#[module(
    imports = [
        HttpModule,
        AuthnModule::for_root(AuthnConfig {
            secret: Some(SECRET.into()),
            ..AuthnConfig::default()
        }),
        SocialModule,
        StubSocialModule,
    ],
    providers = [MembersSignIn, SocialStrategy<MembersSignIn>, SocialGuard, CallbackController],
)]
struct SignInApp;

/// The flow begun at the provider, as the authorize route begins it: the
/// `Cookie` a browser sends back at the callback, and the response's `state`.
fn begun(app: &TestApp, resource: Option<&str>) -> (String, String) {
    let jwt = app
        .container()
        .get::<JwtService>()
        .expect("AuthnModule provides the JwtService");
    let redirect = stub_provider()
        .authorize(&jwt, resource)
        .expect("the flow begins");
    let state = redirect
        .url()
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("state="))
        .expect("the redirect carries its state")
        .to_owned();
    let answered = redirect.into_response();
    let cookie = answered.headers()[header::SET_COOKIE]
        .to_str()
        .expect("ASCII")
        .to_owned();
    (cookie, state)
}

async fn callback(app: &TestApp, query: &str, cookie: Option<&str>) -> TestResponse {
    let request = app.http().get(format!("/social/{KEY}/callback?{query}"));
    match cookie {
        Some(cookie) => request.header("cookie", cookie),
        None => request,
    }
    .send()
    .await
}

fn signed_in(app: &TestApp) -> usize {
    app.container()
        .get::<MembersSignIn>()
        .expect("listed")
        .signed_in
        .load(Ordering::SeqCst)
}

#[tokio::test]
async fn a_matched_response_signs_the_profile_in_for_the_resource_the_flow_asked() {
    let app = TestApp::for_module::<SignInApp>().await.expect("boots");

    for resource in [Some(MCP), None] {
        let (cookie, state) = begun(&app, resource);
        let response = callback(
            &app,
            &format!("code={CODE}&state={state}"),
            Some(&format!("theme=dark; {cookie}")),
        )
        .await;

        response.assert_status_is_ok();
        response
            .assert_json(serde_json::json!({
                "subject": format!("{KEY}:provider-subject-42"),
                "resource": resource,
            }))
            .await;
    }
}

/// RFC 6749 §4.1.2.1: the provider's refusal is answered as what it says,
/// and nothing is exchanged or signed in.
#[tokio::test]
async fn a_provider_refusal_is_refused_as_it_says_and_signs_no_one_in() {
    let app = TestApp::for_module::<SignInApp>().await.expect("boots");
    let (cookie, state) = begun(&app, None);

    for (error, status) in [
        ("access_denied", StatusCode::UNAUTHORIZED),
        ("temporarily_unavailable", StatusCode::SERVICE_UNAVAILABLE),
        ("never-registered", StatusCode::UNAUTHORIZED),
    ] {
        callback(&app, &format!("error={error}&state={state}"), Some(&cookie))
            .await
            .assert_status(status);
    }
    assert_eq!(signed_in(&app), 0);
}

/// A response nobody began here — no cookie, another state, no code — is
/// refused before any exchange.
#[tokio::test]
async fn a_response_no_transaction_matches_is_refused_before_any_exchange() {
    let app = TestApp::for_module::<SignInApp>().await.expect("boots");
    let (cookie, state) = begun(&app, None);

    for (query, cookie) in [
        (format!("code={CODE}&state={state}"), None),
        (
            format!("code={CODE}&state=another-state"),
            Some(cookie.as_str()),
        ),
        (format!("state={state}"), Some(cookie.as_str())),
        (format!("code=&state={state}"), Some(cookie.as_str())),
    ] {
        callback(&app, &query, cookie)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
    assert_eq!(signed_in(&app), 0);
}

/// §4.1.2.1's refusal carries the flow's `state`: one nobody's flow matches
/// is refused as forged, never filed as the provider outage it claims.
#[tokio::test]
async fn an_error_response_no_transaction_matches_is_refused_and_files_no_outage() {
    let app = TestApp::for_module::<SignInApp>().await.expect("boots");
    let (cookie, _) = begun(&app, None);

    for (query, cookie) in [
        ("error=temporarily_unavailable&state=any-state", None),
        (
            "error=temporarily_unavailable&state=another-state",
            Some(cookie.as_str()),
        ),
        ("error=server_error", Some(cookie.as_str())),
    ] {
        let logs = nest_rs_testing::LogCapture::install();

        callback(&app, query, cookie)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        let line = logs.expect_one(nest_rs_authn::TARGET, "guard denied the request");
        assert_eq!(line.field("reason").as_deref(), Some("failed"), "{query}");
    }
    assert_eq!(signed_in(&app), 0);
}

#[tokio::test]
async fn a_provider_no_imported_module_registers_is_refused() {
    let app = TestApp::for_module::<SignInApp>().await.expect("boots");
    let (cookie, state) = begun(&app, None);

    app.http()
        .get(format!("/social/github/callback?code={CODE}&state={state}"))
        .header("cookie", cookie)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(signed_in(&app), 0);
}
