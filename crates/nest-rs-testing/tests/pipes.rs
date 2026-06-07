//! Pipe effectiveness across the three Layer-System scopes — **handler**,
//! **controller**, and **global** (`use_pipes_global`) — plus the
//! `#[no_pipes]` opt-out and the TypeId dedup that guarantees the same
//! pipe declared at multiple scopes still runs exactly once.
//!
//! The pipe under test ([`StripPassword`]) is observable two ways: it
//! strips the `"password"` key from the JSON body the controller echoes
//! back, and it increments a process-global counter on every invocation.
//! Tests share that counter, so a `tokio::sync::Mutex` serializes them —
//! `cargo nextest` parallelizes by default.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::pipe;
use nest_rs_http::{controller, routes};
use nest_rs_pipes::{GlobalPipe, PipeError};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use poem::web::Json;
use serde_json::{Value, json};
use tokio::sync::Mutex;

// --- shared observable state -------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);
// `nextest` runs `#[tokio::test]`s in parallel; the pipe's counter is
// process-global, so each test holds this async gate for its full body.
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

// --- the pipe under test -----------------------------------------------------

/// Strips the `"password"` key from a top-level JSON object body and bumps
/// [`COUNTER`] every invocation. The default-constructed instance is what
/// `#[injectable]` builds; counter sharing is via a static, not via DI.
#[injectable]
#[derive(Default)]
struct StripPassword;

impl Layer for StripPassword {}

impl GlobalPipe for StripPassword {
    fn transform_body(&self, value: &mut Value) -> Result<(), PipeError> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        if let Some(map) = value.as_object_mut() {
            map.remove("password");
        }
        Ok(())
    }
}

// --- controllers, one per scope variant --------------------------------------

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

#[module(providers = [
    StripPassword,
    GlobalScope,
    ControllerScope,
    MethodScope,
    NoPipesScope,
    DupGlobalMethod,
    DupCtrlMethod,
])]
struct PipesModule;

// --- helpers -----------------------------------------------------------------

fn payload() -> Value {
    json!({ "a": 1, "password": "secret" })
}

async fn post_payload(app: &TestApp, path: &str) -> Value {
    let resp = app.http().post(path).body_json(&payload()).send().await;
    resp.assert_status(StatusCode::OK);
    resp.json().await.value().deserialize::<Value>()
}

// --- tests -------------------------------------------------------------------

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

    // Even with a global registration, `#[no_pipes]` on the route must
    // suppress every pipe — global, controller, method.
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

    // Declared globally AND redeclared on the method — the per-route shaper
    // dedups by TypeId so the broadest scope wins; the method redeclaration
    // is dropped. The pipe runs exactly once.
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

    // Same dedup story without globals: controller wins, the method
    // redeclaration is skipped — one execution, body still rewritten.
    let app = TestApp::for_module::<PipesModule>().await.expect("boots");

    let body = post_payload(&app, "/dup-ctrl-method/echo").await;
    assert!(body.get("password").is_none());
    assert_eq!(
        counter(),
        1,
        "TypeId dedup: a pipe at controller + method scope must run once",
    );
}
