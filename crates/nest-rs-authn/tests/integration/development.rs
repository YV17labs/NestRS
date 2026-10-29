//! Covers `src/development.rs` — a test process, which `TestApp` declares,
//! boots an app listing the guard and serves the route it guards; the
//! refusal outside development is the unit tests' table.

use nest_rs_authn::DevelopmentOnlyGuard;
use nest_rs_core::module;
use nest_rs_http::{HttpModule, controller, routes};
use nest_rs_testing::TestApp;

#[controller(path = "/dev")]
#[use_guards(DevelopmentOnlyGuard)]
struct SeedController;

#[routes]
impl SeedController {
    #[post("/seed")]
    #[public]
    async fn seed(&self) -> &'static str {
        "seeded"
    }
}

#[module(
    imports = [HttpModule::for_root(None)],
    providers = [DevelopmentOnlyGuard, SeedController],
)]
struct DevApp;

#[tokio::test]
async fn a_test_process_boots_the_guard_and_serves_its_route() {
    let app = TestApp::for_module::<DevApp>()
        .await
        .expect("a test process boots");

    let response = app.http().post("/dev/seed").send().await;

    response.assert_status_is_ok();
    response.assert_text("seeded").await;
}
