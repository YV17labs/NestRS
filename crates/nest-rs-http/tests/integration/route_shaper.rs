//! What runs at a route before its handler. Its guards read what the route
//! declared — the controller's and the method's `#[meta]` — through the
//! route's one declaration. Its body pipes read the body through the edge's
//! wrapper, like any reader: a read the wrapper stops — past the cap, or
//! stalled — is answered by the edge, `413` or `408`, and files no pipe
//! failure, since the client, not the pipe, ended it.

use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::StreamExt;
use nest_rs_core::{AmbientValue, Layer, UnitContext, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{
    Body, Bytes, Ctx, HttpConfig, HttpModule, HttpView, RawBody, async_trait, controller, routes,
};
use nest_rs_pipes::GlobalPipe;
use nest_rs_testing::{LogCapture, TestApp};
use poem::http::StatusCode;

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
    let opened =
        futures_util::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"{")) });
    let resp = app()
        .await
        .http()
        .post("/piped")
        .content_type("application/json")
        .body(Body::from_stream(
            opened.chain(futures_util::stream::pending()),
        ))
        .send()
        .await;
    resp.assert_status(StatusCode::REQUEST_TIMEOUT);
    logs.expect_none("nest_rs::layers", READ_FAILED);
}

#[derive(Clone, Debug, PartialEq)]
struct Throttle(u32);

#[derive(Clone, Debug, PartialEq)]
struct Audit(&'static str);

static CLONED: AtomicUsize = AtomicUsize::new(0);

/// Counts every copy made of it.
#[derive(Debug)]
struct Counted(u32);

impl Clone for Counted {
    fn clone(&self) -> Self {
        CLONED.fetch_add(1, Ordering::SeqCst);
        Self(self.0)
    }
}

/// What the route's guard read off its declaration, attached for the handler.
#[derive(Clone)]
struct Seen(String);

impl AmbientValue for Seen {}

/// Inserts its own `Throttle` into the request, then reads the declared one.
#[injectable]
#[derive(Default)]
struct Shadowing;

impl Layer for Shadowing {}

#[async_trait]
impl Guard for Shadowing {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let request = HttpView::of(cx)?.request_mut();
        request.extensions_mut().insert(Throttle(1));
        request.extensions_mut().insert(Audit("inserted"));
        let reflector = cx.reflector();
        let seen = format!(
            "{:?} {:?} {:?}",
            reflector.get::<Throttle>(),
            reflector.get::<Audit>(),
            reflector.get::<Counted>().map(|counted| counted.0),
        );
        cx.attach(Seen(seen));
        Ok(())
    }
}

impl GuardFor<HttpView> for Shadowing {}

#[controller(path = "/meta")]
#[use_guards(Shadowing)]
#[meta(Throttle(100))]
#[meta(Audit("controller"))]
struct MetaController;

impl MetaController {
    const QUOTA: u32 = 7;
}

#[routes]
impl MetaController {
    #[get("/declared")]
    #[meta(Throttle(10))]
    #[meta(Counted(3))]
    async fn declared(&self, seen: Ctx<Seen>) -> String {
        seen.into_inner().0
    }

    #[get("/inherited")]
    async fn inherited(&self, seen: Ctx<Seen>) -> String {
        seen.into_inner().0
    }

    #[get("/own")]
    #[meta(Throttle(Self::QUOTA))]
    async fn own(&self, seen: Ctx<Seen>) -> String {
        seen.into_inner().0
    }
}

#[module(providers = [Shadowing, MetaController])]
struct MetaModule;

#[tokio::test]
async fn the_methods_value_wins_and_a_host_value_of_another_type_survives() {
    let client = crate::boot::<MetaModule>().await;

    let declared = client.get("/meta/declared").send().await;
    declared.assert_status_is_ok();
    declared
        .assert_text(r#"Some(Throttle(10)) Some(Audit("controller")) Some(3)"#)
        .await;

    let inherited = client.get("/meta/inherited").send().await;
    inherited.assert_status_is_ok();
    inherited
        .assert_text(r#"Some(Throttle(100)) Some(Audit("controller")) None"#)
        .await;
}

#[tokio::test]
async fn a_methods_value_names_its_host_through_self() {
    let client = crate::boot::<MetaModule>().await;

    let own = client.get("/meta/own").send().await;
    own.assert_status_is_ok();
    own.assert_text(r#"Some(Throttle(7)) Some(Audit("controller")) None"#)
        .await;
}

#[tokio::test]
async fn a_value_a_guard_inserts_into_the_request_never_shadows_the_declared_one() {
    let client = crate::boot::<MetaModule>().await;

    let declared = client.get("/meta/declared").send().await;
    declared.assert_status_is_ok();
    let text = declared.0.into_body().into_string().await.expect("a body");
    assert!(
        text.starts_with("Some(Throttle(10))"),
        "the guard's `Throttle(1)` was read: {text}",
    );
}

#[tokio::test]
async fn a_declared_value_is_never_copied_per_request() {
    let client = crate::boot::<MetaModule>().await;
    let before = CLONED.load(Ordering::SeqCst);

    for _ in 0..3 {
        client
            .get("/meta/declared")
            .send()
            .await
            .assert_status_is_ok();
    }

    assert_eq!(CLONED.load(Ordering::SeqCst), before, "a value was cloned");
}
