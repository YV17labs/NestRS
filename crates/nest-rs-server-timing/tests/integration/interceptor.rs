//! The documented import, booted, and the header a caller reads back: the
//! interceptor is mounted by the import, so only a boot can see it.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use nest_rs_server_timing::{ServerTimingModule, Timings};
use nest_rs_testing::TestApp;

#[controller(path = "/")]
struct ProbeController;

#[routes]
impl ProbeController {
    /// Records one sub-step the documented way, through [`Timings`].
    #[get("/")]
    #[public]
    async fn index(&self, req: &nest_rs_http::poem::Request) -> String {
        if let Some(timings) = req.extensions().get::<Timings>() {
            timings.record("db", Duration::from_millis(3));
        }
        "ok".into()
    }
}

#[module(imports = [ServerTimingModule], providers = [ProbeController])]
struct AppModule;

#[tokio::test]
async fn the_documented_import_puts_the_header_on_a_response() {
    let app = TestApp::for_module::<AppModule>()
        .await
        .expect("the documented wiring boots");

    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    let header = resp
        .0
        .headers()
        .get("server-timing")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .expect("the import is what attaches the header");

    // W3C Server-Timing §3: a comma-separated list of metrics, each a name with
    // optional `;dur=` and `;desc=` parameters.
    assert!(
        header.contains("db;dur="),
        "the sub-step a handler recorded reaches the wire: {header}",
    );
}
