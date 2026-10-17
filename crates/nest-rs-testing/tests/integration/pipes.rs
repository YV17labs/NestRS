//! Pipe effectiveness across the three scopes — **handler**, **controller**
//! and **global** — plus the `#[no_pipes]` opt-out, the `TypeId` dedup, and
//! what a pipe is handed on a route: the body, as the route's unit.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Edge, Layer, UnitContext, injectable, module};
use nest_rs_http::Json;
use nest_rs_http::StatusCode;
use nest_rs_http::{HttpView, controller, routes};
use nest_rs_pipes::{ArgumentKind, GlobalPipe, PipeError, pipe};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};
use tokio::sync::Mutex;

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

/// Strips the `"password"` key from a top-level JSON object body and bumps
/// [`COUNTER`] every invocation.
#[injectable]
#[derive(Default)]
struct StripPassword;

impl Layer for StripPassword {}

impl GlobalPipe for StripPassword {
    fn transform(
        &self,
        _cx: &UnitContext<'_>,
        argument: ArgumentKind,
        value: &mut Value,
    ) -> Result<(), PipeError> {
        assert_eq!(argument, ArgumentKind::Body, "a route hands its body");
        COUNTER.fetch_add(1, Ordering::SeqCst);
        if let Some(map) = value.as_object_mut() {
            map.remove("password");
        }
        Ok(())
    }
}

#[controller(path = "/global")]
struct GlobalScope;

#[routes]
impl GlobalScope {
    #[post("/echo")]
    async fn echo_global(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

#[controller(path = "/ctrl")]
#[use_pipes(StripPassword)]
struct ControllerScope;

#[routes]
impl ControllerScope {
    #[post("/echo")]
    async fn echo_ctrl(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

#[controller(path = "/method")]
struct MethodScope;

#[routes]
impl MethodScope {
    #[post("/echo")]
    #[use_pipes(StripPassword)]
    async fn echo_method(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

#[controller(path = "/no-pipes")]
struct NoPipesScope;

#[routes]
impl NoPipesScope {
    #[post("/echo")]
    #[no_pipes]
    async fn echo_no_pipes(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

#[controller(path = "/dup-global-method")]
struct DupGlobalMethod;

#[routes]
impl DupGlobalMethod {
    #[post("/echo")]
    #[use_pipes(StripPassword)]
    async fn echo_dup_global_method(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

#[controller(path = "/dup-ctrl-method")]
#[use_pipes(StripPassword)]
struct DupCtrlMethod;

#[routes]
impl DupCtrlMethod {
    #[post("/echo")]
    #[use_pipes(StripPassword)]
    async fn echo_dup_ctrl_method(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

/// What one pipe call was handed: the input, the unit's edge, the site's
/// address, and the matched route the view holds.
type Seen = (ArgumentKind, Option<Edge>, &'static str, Option<String>);

static SEEN: std::sync::Mutex<Vec<Seen>> = std::sync::Mutex::new(Vec::new());

/// Records what it is handed, and changes nothing.
#[injectable]
#[derive(Default)]
struct RecordingPipe;

impl Layer for RecordingPipe {}

impl GlobalPipe for RecordingPipe {
    fn transform(
        &self,
        cx: &UnitContext<'_>,
        argument: ArgumentKind,
        _value: &mut Value,
    ) -> Result<(), PipeError> {
        let route = cx
            .switch_to::<HttpView>()
            .and_then(HttpView::route)
            .map(str::to_owned);
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((argument, cx.edge(), cx.handler().address(), route));
        Ok(())
    }
}

#[controller(path = "/recorded")]
struct RecordedScope;

#[routes]
impl RecordedScope {
    #[post("/echo/{id}")]
    #[use_pipes(RecordingPipe)]
    async fn echo_recorded(&self, body: Json<Value>) -> Json<Value> {
        Json(body.0)
    }
}

#[module(providers = [
    RecordingPipe,
    RecordedScope,
    StripPassword,
    GlobalScope,
    ControllerScope,
    MethodScope,
    NoPipesScope,
    DupGlobalMethod,
    DupCtrlMethod,
])]
struct PipesModule;

fn payload() -> Value {
    json!({ "a": 1, "password": "secret" })
}

async fn post_payload(app: &TestApp, path: &str) -> Value {
    let resp = app.http().post(path).body_json(&payload()).send().await;
    resp.assert_status(StatusCode::OK);
    resp.json().await.value().deserialize::<Value>()
}

#[tokio::test]
async fn pipe_at_global_scope_strips_field() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<PipesModule>()
        .use_pipes_global([pipe::<StripPassword>()])
        .build()
        .await
        .expect("boots");

    let body = post_payload(&app, "/global/echo").await;
    assert!(
        body.get("password").is_none(),
        "global pipe should have stripped `password`, got {body}",
    );
    assert_eq!(body.get("a"), Some(&json!(1)));
    assert_eq!(counter(), 1, "the global pipe ran exactly once");
}

#[tokio::test]
async fn pipe_at_controller_scope_strips_field() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<PipesModule>().await.expect("boots");

    let body = post_payload(&app, "/ctrl/echo").await;
    assert!(
        body.get("password").is_none(),
        "controller-scope pipe should have stripped `password`, got {body}",
    );
    assert_eq!(body.get("a"), Some(&json!(1)));
    assert_eq!(counter(), 1, "the controller-scope pipe ran exactly once");
}

#[tokio::test]
async fn pipe_at_method_scope_strips_field() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<PipesModule>().await.expect("boots");

    let body = post_payload(&app, "/method/echo").await;
    assert!(
        body.get("password").is_none(),
        "method-scope pipe should have stripped `password`, got {body}",
    );
    assert_eq!(body.get("a"), Some(&json!(1)));
    assert_eq!(counter(), 1, "the method-scope pipe ran exactly once");
}

#[tokio::test]
async fn no_pipes_skips_all_pipes() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<PipesModule>()
        .use_pipes_global([pipe::<StripPassword>()])
        .build()
        .await
        .expect("boots");

    let body = post_payload(&app, "/no-pipes/echo").await;
    assert_eq!(
        body.get("password"),
        Some(&json!("secret")),
        "`#[no_pipes]` must keep the body intact, got {body}",
    );
    assert_eq!(counter(), 0, "no pipe should have run");
}

#[tokio::test]
async fn same_pipe_global_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<PipesModule>()
        .use_pipes_global([pipe::<StripPassword>()])
        .build()
        .await
        .expect("boots");

    let body = post_payload(&app, "/dup-global-method/echo").await;
    assert!(body.get("password").is_none());
    assert_eq!(
        counter(),
        1,
        "TypeId dedup: a pipe global + method-declared must still run once",
    );
}

#[tokio::test]
async fn same_pipe_controller_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<PipesModule>().await.expect("boots");

    let body = post_payload(&app, "/dup-ctrl-method/echo").await;
    assert!(body.get("password").is_none());
    assert_eq!(
        counter(),
        1,
        "TypeId dedup: a pipe at controller + method scope must run once",
    );
}

#[tokio::test]
async fn same_pipe_at_all_three_scopes_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<PipesModule>()
        .use_pipes_global([pipe::<StripPassword>()])
        .build()
        .await
        .expect("boots");

    let body = post_payload(&app, "/dup-ctrl-method/echo").await;
    assert!(body.get("password").is_none());
    assert_eq!(
        counter(),
        1,
        "global + controller + method pipe declaration still executes exactly once",
    );
}

#[tokio::test]
async fn a_pipe_on_a_route_is_handed_the_body_as_the_routes_unit() {
    let app = TestApp::for_module::<PipesModule>().await.expect("boots");

    let resp = app
        .http()
        .post("/recorded/echo/7")
        .body_json(&payload())
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let seen = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        seen,
        [(
            ArgumentKind::Body,
            Some(Edge::Http),
            "/echo/{id}",
            Some("/recorded/echo/{id}".to_owned()),
        )],
        "one call, on the route's body, with the route's handler and view",
    );
}
