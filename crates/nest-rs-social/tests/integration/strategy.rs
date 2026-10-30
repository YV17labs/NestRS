//! Covers `src/strategy.rs` — the redirect URI's RFC 6749 §4.1.2 response, read
//! against the transaction that began the flow, signs the provider's profile in
//! through the product's `SocialSignIn`, carrying the resource the flow asked
//! for; §4.1.2.1's refusal and a response nobody began never reach it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authn::{
    AuthError, AuthnConfig, AuthnGuard, AuthnModule, JwtService, PrincipalIdentity,
};
use nest_rs_core::{AmbientValue, Ctx, injectable, module};
use nest_rs_http::{HttpModule, Json, StatusCode, async_trait, controller, routes};
use nest_rs_oauth_client::{
    AuthorizationRedirect, AuthorizationResponse, OAuthClient, OAuthClientConfig, TokenSet,
};
use nest_rs_social::{
    ProfileFuture, SocialModule, SocialProfile, SocialProvider, SocialProviderEntry, SocialSignIn,
    SocialStrategy, TokenFuture,
};
use nest_rs_testing::{TestApp, TestResponse};

const KEY: &str = "callback-stub";
const SECRET: &str = "social-strategy-secret-padded-32b";
const CODE: &str = "the-code-never-logged";
const MCP: &str = "https://mcp.example.com/mcp";

/// A provider whose exchange and profile answer in process: what the strategy
/// hands it, it checks.
struct StubProvider {
    client: OAuthClient,
}

impl SocialProvider for StubProvider {
    fn key(&self) -> &'static str {
        KEY
    }

    fn client(&self) -> &OAuthClient {
        &self.client
    }

    fn exchange(&self, response: AuthorizationResponse) -> TokenFuture<'_> {
        Box::pin(async move {
            assert_eq!(response.code().expose_secret(), CODE);
            Ok(TokenSet::new("provider-access-token"))
        })
    }

    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move {
            assert_eq!(tokens.access_token.expose_secret(), "provider-access-token");
            Ok(SocialProfile::new(KEY, "provider-subject-42"))
        })
    }
}

fn stub_provider() -> StubProvider {
    StubProvider {
        client: OAuthClient::new(OAuthClientConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://provider.example/authorize".into(),
            token_url: "https://provider.example/token".into(),
            redirect_url: "https://app.example/social/callback-stub/callback".into(),
            userinfo_url: "https://provider.example/userinfo".into(),
            scopes: vec!["read".into()],
            tls: nest_rs_config::ClientTls::default(),
        })
        .expect("a valid client"),
    }
}

#[module]
struct StubSocialModule;

nest_rs_core::inventory::submit! {
    SocialProviderEntry::new::<StubProvider, StubSocialModule>(
        KEY,
        "SOCIAL__CALLBACK_STUB",
        |_container| Ok(Some(Arc::new(stub_provider()))),
    )
}

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
    async fn callback(&self, member: Ctx<Member>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "subject": member.subject,
            "resource": member.resource,
        }))
    }
}

#[module(
    imports = [
        HttpModule::for_root(None),
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

/// The flow begun at the provider, as the authorize route begins it.
fn begun(app: &TestApp, resource: Option<&str>) -> (AuthorizationRedirect, String) {
    let jwt = app
        .container()
        .get::<JwtService>()
        .expect("AuthnModule provides the JwtService");
    let redirect = stub_provider()
        .authorize(&jwt, resource)
        .expect("the flow begins");
    let state = redirect
        .url
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("state="))
        .expect("the redirect carries its state")
        .to_owned();
    (redirect, state)
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
        let (redirect, state) = begun(&app, resource);
        let response = callback(
            &app,
            &format!("code={CODE}&state={state}"),
            Some(&format!("theme=dark; {}", redirect.set_cookie())),
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
    let (redirect, state) = begun(&app, None);
    let cookie = redirect.set_cookie();

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
    let (redirect, state) = begun(&app, None);
    let cookie = redirect.set_cookie();

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
    let (redirect, _) = begun(&app, None);
    let cookie = redirect.set_cookie();

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
    let (redirect, state) = begun(&app, None);

    app.http()
        .get(format!("/social/github/callback?code={CODE}&state={state}"))
        .header("cookie", redirect.set_cookie())
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(signed_in(&app), 0);
}
