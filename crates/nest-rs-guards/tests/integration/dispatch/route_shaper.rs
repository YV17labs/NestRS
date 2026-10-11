//! The route's body pipes read the body through the edge's wrapper, like any
//! reader: a read the wrapper stops — past the cap, or stalled — is answered by
//! the edge, `413` or `408`, and files no pipe failure, since the client, not
//! the pipe, ended it.

use futures_util::StreamExt;
use nest_rs_core::{Layer, injectable, module};
use nest_rs_http::poem::Body;
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::{HttpConfig, HttpModule, RawBody, controller, routes};
use nest_rs_pipes::GlobalPipe;
use nest_rs_testing::{LogCapture, TestApp};

const CAP: usize = 64;

/// What a pipe's own read failure files.
const READ_FAILED: &str = "global pipe: failed to read body";

/// Runs on every JSON body the route takes, changing nothing.
#[injectable]
#[derive(Default)]
struct KeepBody;

impl Layer for KeepBody {}

impl GlobalPipe for KeepBody {}

#[controller(path = "/piped")]
struct PipedController;

#[routes]
impl PipedController {
    #[post("/")]
    #[use_pipes(KeepBody)]
    async fn take(&self, body: RawBody) -> String {
        format!("{} bytes", body.len())
    }
}

#[module(
    imports = [HttpModule::for_root(HttpConfig { max_body_bytes: Some(CAP), ..Default::default() })],
    providers = [KeepBody, PipedController],
)]
struct PipedModule;

async fn app() -> TestApp {
    TestApp::for_module::<PipedModule>()
        .await
        .expect("a route with a body pipe boots")
}

#[tokio::test]
async fn a_body_past_the_cap_is_the_edges_413_and_no_pipe_failure() {
    let logs = LogCapture::install();
    let resp = app()
        .await
        .http()
        .post("/piped")
        .content_type("application/json")
        .body(vec![b' '; CAP + 1])
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    logs.expect_none("nest_rs::layers", READ_FAILED);
}

#[tokio::test(start_paused = true)]
async fn a_body_that_stops_arriving_is_the_edges_408_and_no_pipe_failure() {
    let logs = LogCapture::install();
    let opened = futures_util::stream::once(async { Ok::<_, std::io::Error>(b"{".to_vec()) });
    let resp = app()
        .await
        .http()
        .post("/piped")
        .content_type("application/json")
        .body(Body::from_bytes_stream(
            opened.chain(futures_util::stream::pending()),
        ))
        .send()
        .await;
    resp.assert_status(StatusCode::REQUEST_TIMEOUT);
    logs.expect_none("nest_rs::layers", READ_FAILED);
}
