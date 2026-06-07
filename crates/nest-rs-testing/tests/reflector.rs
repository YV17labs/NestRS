//! `#[meta(...)]` + `Reflector`: guard reads route metadata (the `@Roles`
//! pattern), end-to-end through the HTTP harness.

use nest_rs_core::{HandlerMetadata, Layer, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{Reflector, async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::Request;
use poem::http::StatusCode;

#[derive(Clone)]
struct RequiredRoles(&'static [&'static str]);

#[injectable]
#[derive(Default)]
struct RolesGuard;

impl Layer for RolesGuard {}

#[async_trait]
impl Guard for RolesGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let required = Reflector::new(req)
            .get::<RequiredRoles>()
            .map(|r| r.0)
            .unwrap_or(&[]);
        let caller = req
            .headers()
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if required.is_empty() || required.contains(&caller) {
            Ok(())
        } else {
            Err(Denial::forbidden("forbidden"))
        }
    }
}

#[controller(path = "/")]
struct AdminController;

#[routes]
impl AdminController {
    #[get("/admin")]
    #[use_guards(RolesGuard)]
    #[meta(RequiredRoles(&["admin"]))]
    async fn admin(&self) -> &'static str {
        "secret"
    }
}

#[module(providers = [RolesGuard, AdminController])]
struct AdminModule;

#[tokio::test]
async fn guard_allows_when_caller_role_matches_route_metadata() {
    let app = TestApp::for_module::<AdminModule>().await.expect("boots");
    let resp = app
        .http()
        .get("/admin")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("secret").await;
}

#[tokio::test]
async fn guard_rejects_when_caller_role_is_insufficient() {
    let app = TestApp::for_module::<AdminModule>().await.expect("boots");
    let resp = app
        .http()
        .get("/admin")
        .header("x-role", "user")
        .send()
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn guard_rejects_when_role_header_is_absent() {
    let app = TestApp::for_module::<AdminModule>().await.expect("boots");
    let resp = app.http().get("/admin").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
}
