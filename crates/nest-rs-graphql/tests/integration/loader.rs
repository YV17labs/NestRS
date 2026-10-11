//! `#[dataloader]` — the batch a generated loader runs is the owner's method.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_core::{AmbientValue, ambient, injectable, module, with_ambient};
use nest_rs_graphql::async_graphql::dataloader::{DataLoader, Loader};
use nest_rs_graphql::async_graphql::{self, Context};
use nest_rs_graphql::{
    BoxFuture, GraphqlModule, GraphqlOperationGuard, dataloader, operations, resolver,
};
use nest_rs_http::poem;
use nest_rs_http::{HttpConfig, HttpModule};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A trait whose method shares the batch's name, implemented for the `Arc` the
/// loader holds its owner in: method syntax finds it before it derefs.
#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait LabelsOnArc {
    fn labels(&self, keys: &[i32]) -> HashMap<i32, String>;
}

impl<T> LabelsOnArc for Arc<T> {
    fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        keys.iter()
            .map(|key| (*key, "the trait on Arc".into()))
            .collect()
    }
}

#[injectable]
#[derive(Default)]
struct Shelf;

#[dataloader]
impl Shelf {
    async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        keys.iter()
            .map(|key| (*key, format!("shelf {key}")))
            .collect()
    }
}

#[tokio::test]
async fn a_loader_runs_its_owners_method_and_not_a_trait_method_of_the_same_name() {
    let loaded = ShelfLabels(Arc::new(Shelf)).load(&[7]).await;
    let Ok(loaded) = loaded;
    assert_eq!(loaded.get(&7).map(String::as_str), Some("shelf 7"));
}

/// Who the request is for, as an ability bridge says what the caller may do.
#[derive(Debug)]
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// Adds the caller around the request's execution, as the authorization
/// bridge installs the caller's ability.
#[injectable]
#[derive(Default)]
struct CallerBridge;

impl GraphqlOperationGuard for CallerBridge {
    fn before<'a>(
        &'a self,
        _req: &'a mut poem::Request,
    ) -> BoxFuture<'a, Result<(), poem::Response>> {
        Box::pin(async { Ok(()) })
    }

    fn around<'a>(
        &'a self,
        _req: &'a poem::Request,
        inner: BoxFuture<'a, ()>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(with_ambient(Arc::new(Caller("alice")), inner))
    }
}

/// Answers each key with what its batch ran under.
#[injectable]
#[derive(Default)]
struct TraceShelf;

#[dataloader]
impl TraceShelf {
    async fn traces(&self, keys: &[i32]) -> HashMap<i32, String> {
        let seen = nest_rs_core::current_trace_id()
            .map(|trace| trace.to_hex())
            .unwrap_or_default();
        keys.iter().map(|key| (*key, seen.clone())).collect()
    }

    async fn callers(&self, keys: &[i32]) -> HashMap<i32, String> {
        let seen = ambient::<Caller>().map_or("nobody", |caller| caller.0);
        keys.iter().map(|key| (*key, seen.to_owned())).collect()
    }
}

#[resolver]
struct TraceShelfResolver;

#[operations]
impl TraceShelfResolver {
    #[query]
    #[public]
    async fn batch_trace(&self, ctx: &Context<'_>, id: i32) -> async_graphql::Result<String> {
        let loader = ctx.data::<DataLoader<TraceShelfTraces>>()?;
        let Ok(found) = loader.load_one(id).await;
        Ok(found.unwrap_or_default())
    }

    #[query]
    #[public]
    async fn batch_caller(&self, ctx: &Context<'_>, id: i32) -> async_graphql::Result<String> {
        let loader = ctx.data::<DataLoader<TraceShelfCallers>>()?;
        let Ok(found) = loader.load_one(id).await;
        Ok(found.unwrap_or_default())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        TraceShelf,
        TraceShelfResolver,
        CallerBridge as dyn GraphqlOperationGuard,
    ],
)]
struct TraceShelfApp;

#[tokio::test]
async fn a_batch_runs_under_the_trace_of_the_request_that_triggered_it() {
    let app = nest_rs_testing::TestApp::for_module::<TraceShelfApp>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ batchTrace(id: 7) }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let request_trace = resp
        .0
        .headers()
        .get(nest_rs_http::TRACERESPONSE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('-').nth(1))
        .map(str::to_owned)
        .expect("the edge answers the trace it served the request under");
    let body: serde_json::Value = resp.json().await.value().deserialize();

    assert_eq!(
        body["data"]["batchTrace"].as_str(),
        Some(request_trace.as_str()),
        "the batch reads the request's trace id: {body}",
    );
}

#[tokio::test]
async fn a_batch_reads_what_the_request_carries_with_no_batch_binding() {
    let app = nest_rs_testing::TestApp::for_module::<TraceShelfApp>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(
            &serde_json::json!({ "query": "{ a: batchCaller(id: 1) b: batchCaller(id: 2) }" }),
        )
        .send()
        .await;
    resp.assert_status_is_ok();
    let body: serde_json::Value = resp.json().await.value().deserialize();

    assert_eq!(body["data"]["a"].as_str(), Some("alice"), "{body}");
    assert_eq!(
        body["data"]["b"].as_str(),
        Some("alice"),
        "one batch serves both fields: {body}",
    );
}

/// Set when the slow batch is dropped, finished and started, in turn.
static SLOW_BATCH_DROPPED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SLOW_BATCH_FINISHED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SLOW_BATCH_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct SetOnDrop(&'static std::sync::atomic::AtomicBool);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[injectable]
#[derive(Default)]
struct SlowShelf;

#[dataloader]
impl SlowShelf {
    async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        let _dropped = SetOnDrop(&SLOW_BATCH_DROPPED);
        SLOW_BATCH_STARTED.notify_one();
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        SLOW_BATCH_FINISHED.store(true, std::sync::atomic::Ordering::SeqCst);
        keys.iter().map(|key| (*key, "late".into())).collect()
    }
}

#[resolver]
struct SlowShelfResolver;

#[operations]
impl SlowShelfResolver {
    #[query]
    #[public]
    async fn label(&self, ctx: &Context<'_>, id: i32) -> async_graphql::Result<String> {
        let loader = ctx.data::<DataLoader<SlowShelfLabels>>()?;
        let Ok(found) = loader.load_one(id).await;
        Ok(found.unwrap_or_default())
    }
}

#[module(
    imports = [
        GraphqlModule::for_root(None),
        HttpModule::for_root(HttpConfig {
            shutdown_timeout: std::time::Duration::from_secs(1),
            ..Default::default()
        }),
    ],
    providers = [SlowShelf, SlowShelfResolver],
)]
struct SlowShelfApp;

#[tokio::test]
async fn a_batch_still_running_when_the_transport_stops_is_dropped_with_it() {
    let app = nest_rs_testing::TestApp::builder()
        .module::<SlowShelfApp>()
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut client = connect(app.addr()).await;
    let body = r#"{"query":"{ label(id: 7) }"}"#;
    client
        .write_all(
            format!(
                "POST /graphql HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\n\r\n{body}",
                body.len(),
            )
            .as_bytes(),
        )
        .await
        .expect("the request is sent");
    SLOW_BATCH_STARTED.notified().await;

    tokio::time::pause();
    app.shutdown().await.expect("the transport stops cleanly");
    tokio::time::resume();

    assert!(
        SLOW_BATCH_DROPPED.load(std::sync::atomic::Ordering::SeqCst)
            && !SLOW_BATCH_FINISHED.load(std::sync::atomic::Ordering::SeqCst),
        "the batch was dropped where it waited before the transport returned",
    );
    let mut rest = Vec::new();
    let _ = client.read_to_end(&mut rest).await;
    assert!(rest.is_empty(), "the cut request was not answered");
}

/// A connection to the served address, retried while the listener comes up.
async fn connect(addr: std::net::SocketAddr) -> tokio::net::TcpStream {
    for _ in 0..100 {
        if let Ok(stream) = tokio::net::TcpStream::connect(addr).await {
            return stream;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the transport never came up on {addr}");
}
