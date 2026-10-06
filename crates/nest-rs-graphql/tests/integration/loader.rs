//! `#[dataloader]` — the batch a generated loader runs is the owner's method.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_core::{injectable, module};
use nest_rs_graphql::async_graphql::dataloader::{DataLoader, Loader};
use nest_rs_graphql::async_graphql::{self, Context};
use nest_rs_graphql::{GraphqlModule, dataloader, operations, resolver};
use nest_rs_http::{HttpConfig, HttpModule};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A trait whose method shares the batch's name, implemented for the `Arc` the
/// loader holds its owner in. Method syntax on that `Arc` finds this before it
/// derefs to the owner, so a loader calling `self.0.labels(..)` ran this body.
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

/// async-graphql runs every DataLoader batch on a task of its own, so a batch
/// in flight when the shutdown window cut its request ran on after the
/// transport returned — through the shutdown hooks, its reads landing after
/// `OnModuleDestroy`, with nobody left to read them. It is work the connection
/// only carried, so it stops with the transport: dropped before `serve` returns.
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

    // The window passes on paused time: nothing reads the connection meanwhile.
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
