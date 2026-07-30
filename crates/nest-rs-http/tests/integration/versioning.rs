//! URI versioning — and the layout the versioning page prescribes for it.
//!
//! > "Both controllers live in the feature's `http/controller.rs` — a version
//! > is a wire concern, not a second feature."
//!
//! Two versions of one route, in **one file**, with the **same handler name**,
//! is therefore *the* documented shape. It did not compile: `#[routes]` emits a
//! module-level type per handler (poem's `#[handler]` form), and the symbol was
//! derived from the method name alone — so `V1Controller::ping` and
//! `V2Controller::ping` collided in a namespace neither knew it shared. The
//! error named the mangled symbol and nothing connected it to "rename one of
//! your handlers". Nothing about it was versioning-specific either: `list` /
//! `get` / `create` are exactly the names two controllers in one file repeat.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, controller, routes};
use poem::test::TestClient;

#[controller(path = "/fund", version = "1")]
struct FundV1Controller;

#[routes]
impl FundV1Controller {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "v1".into()
    }
}

// Same handler name, same file — only the `Json<T>` shape differs in the real
// pattern the page describes, so the method names are expected to match.
#[controller(path = "/fund", version = "2")]
struct FundV2Controller;

#[routes]
impl FundV2Controller {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "v2".into()
    }
}

// A third controller with no version and the same handler name again: the
// collision is about sharing a file, not about versioning.
#[controller(path = "/unversioned")]
struct UnversionedController;

#[routes]
impl UnversionedController {
    #[get("/ping")]
    async fn ping(&self) -> String {
        "none".into()
    }
}

#[module(providers = [FundV1Controller, FundV2Controller, UnversionedController])]
struct VersionedModule;

#[tokio::test]
async fn two_controllers_in_one_file_may_share_a_handler_name() {
    let app = App::builder()
        .module::<VersionedModule>()
        .build()
        .await
        .expect("boots");
    let mut transport = HttpTransport::new();
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    let client = TestClient::new(
        transport
            .take_endpoint()
            .expect("configure populates the endpoint"),
    );

    for (path, body) in [
        ("/v1/fund/ping", "v1"),
        ("/v2/fund/ping", "v2"),
        ("/unversioned/ping", "none"),
    ] {
        let resp = client.get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }

    // The unversioned form of a versioned controller is not mounted.
    client
        .get("/fund/ping")
        .send()
        .await
        .assert_status(poem::http::StatusCode::NOT_FOUND);
}
