//! Boot a module's DI graph in-process and drive HTTP, including a `dyn`
//! provider override.

use std::sync::Arc;

use nestrs_core::{injectable, module};
use nestrs_http::{controller, routes};
use nestrs_testing::TestApp;

trait Greeter: Send + Sync {
    fn greet(&self) -> String;
}

#[injectable]
#[derive(Default)]
struct RealGreeter;

impl Greeter for RealGreeter {
    fn greet(&self) -> String {
        "Hello World".into()
    }
}

/// A fake bound to the same trait, swapped in via `override_dyn`.
struct MockGreeter;

impl Greeter for MockGreeter {
    fn greet(&self) -> String {
        "Mocked".into()
    }
}

#[controller(path = "/")]
struct GreeterController {
    #[inject]
    greeter: Arc<dyn Greeter>,
}

#[routes]
impl GreeterController {
    #[get("/")]
    async fn hello(&self) -> String {
        self.greeter.greet()
    }
}

#[module(providers = [RealGreeter as dyn Greeter, GreeterController])]
struct GreeterModule;

#[tokio::test]
async fn boots_and_serves_the_real_provider() {
    let app = TestApp::for_module::<GreeterModule>()
        .await
        .expect("the module boots");
    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Hello World").await;
}

#[tokio::test]
async fn override_dyn_swaps_in_a_mock_seen_by_the_controller() {
    let app = TestApp::builder()
        .module::<GreeterModule>()
        .override_dyn::<dyn Greeter>(Arc::new(MockGreeter))
        .build()
        .await
        .expect("the module boots with the override");
    // The controller is built from the final container at mount, so it resolves
    // the overridden binding — the response proves the swap reached the handler.
    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Mocked").await;
}

#[tokio::test]
async fn container_exposes_the_overridden_binding() {
    let app = TestApp::builder()
        .module::<GreeterModule>()
        .override_dyn::<dyn Greeter>(Arc::new(MockGreeter))
        .build()
        .await
        .expect("the module boots with the override");
    let greeter = app
        .container()
        .get_dyn::<dyn Greeter>()
        .expect("dyn Greeter is registered");
    assert_eq!(greeter.greet(), "Mocked");
}
