//! The line a request files on `nest_rs::operation`.

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use nest_rs_testing::LogCapture;

use crate::boot;

#[controller(path = "/greetings")]
struct GreetingsController;

#[routes]
impl GreetingsController {
    #[get("/")]
    #[public]
    async fn greet(&self) -> &'static str {
        "hello"
    }
}

#[module(providers = [GreetingsController])]
struct GreetingsModule;

/// The line names no client: an address is personal data, and a line is kept
/// wherever the deployment ships its logs. Neither the address nor where it was
/// read from is written.
#[tokio::test]
async fn the_line_carries_no_client_address() {
    let logs = LogCapture::install();
    let client = boot::<GreetingsModule>().await;

    client.get("/greetings").send().await.assert_status_is_ok();

    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    assert_eq!(line.field("client_ip"), None, "{:?}", line.fields);
    assert_eq!(line.field("forwarded"), None, "{:?}", line.fields);
}
