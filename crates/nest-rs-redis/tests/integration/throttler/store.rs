//! `RedisThrottler` (`src/throttler/store.rs`) under the throttler's guard,
//! over a Redis that stops: the store answers its error and says nothing, the
//! guard denies for the window and files the one line.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
use nest_rs_core::{App, Handler, Posture, module};
use nest_rs_guards::Guard;
use nest_rs_http::__private::check_request;
use nest_rs_redis::{RedisConfig, RedisModule, RedisThrottler, RedisThrottlerModule};
use nest_rs_throttler::{ThrottlerConfig, ThrottlerGuard, ThrottlerModule, ThrottlerSetup};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::harness::connection::answer_each_command;
use crate::{HELLO, is_hello};

/// A Redis serving the boot — its `HELLO` as a primary's, `+PONG` to the rest
/// — until [`stop`](Self::stop): then every connection closes at its next
/// command and every new one at once, as a server gone down does.
struct StoppingRedis {
    addr: SocketAddr,
    stopped: Arc<AtomicBool>,
}

impl StoppingRedis {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the server");
        let addr = listener.local_addr().expect("the server's address");
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stopped);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                if stopping.load(Ordering::SeqCst) {
                    continue;
                }
                let stopping = Arc::clone(&stopping);
                tokio::spawn(async move {
                    if let Some(client) = crate::harness::tls::accept(client).await {
                        serve_until_stopped(client, &stopping).await;
                    }
                });
            }
        });
        Self { addr, stopped }
    }

    fn url(&self) -> String {
        format!("rediss://{}/", self.addr)
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

async fn serve_until_stopped(client: impl AsyncRead + AsyncWrite + Unpin, stopped: &AtomicBool) {
    answer_each_command(client, |command| {
        if stopped.load(Ordering::SeqCst) {
            return None;
        }
        let reply: &[u8] = if is_hello(command) {
            HELLO
        } else {
            b"+PONG\r\n"
        };
        Some(reply)
    })
    .await;
}

/// The window the app counts in, and so the wait a refusal carries.
const WINDOW_SECS: u64 = 30;

fn policy() -> ThrottlerSetup {
    ThrottlerModule::for_root(ThrottlerConfig {
        limit: Some(60),
        window_secs: Some(WINDOW_SECS),
        pseudonym_key: Some("nest-rs-redis integration pseudonym key".to_owned()),
    })
}

#[module(imports = [RedisThrottlerModule, policy(), RedisModule::for_root(None)])]
struct RedisThrottlerHost;

struct Routes;

/// The route the request below reaches.
static RATED: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: std::any::TypeId::of::<Routes>(),
    host_name: "Routes",
    method: "rated",
    address: "/rated",
    posture: Posture::Public,
    meta: MetaLevels::default,
});

/// The line the guard files for a store that could not count.
const FAILED: &str = "throttler store failed; denying (fail-closed)";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_that_stops_denies_with_one_line_from_the_guard_and_none_from_the_store() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let server = StoppingRedis::start().await;
    let app = App::builder()
        .provide(RedisConfig {
            url: server.url(),
            connect_timeout: Duration::from_secs(1),
            tls: crate::harness::tls::trusting_the_test_authority(),
            ..RedisConfig::default()
        })
        .module::<RedisThrottlerHost>()
        .build()
        .await
        .expect("the server serves the boot");
    let guard = app
        .container()
        .get::<ThrottlerGuard>()
        .expect("ThrottlerModule registers the guard");
    server.stop();

    let mut request = nest_rs_http::Request::builder()
        .uri(nest_rs_http::Uri::from_static("/rated"))
        .finish();
    let refused = check_request(
        &RATED,
        [("ThrottlerGuard", &*guard as &dyn Guard)],
        &mut request,
    )
    .await
    .expect_err("a store that cannot count refuses the request");

    assert_eq!(refused.http_status(), 429);
    let said = logs.expect_one(nest_rs_throttler::TARGET, FAILED);
    assert_eq!(said.level, "warn");
    assert_eq!(
        said.field("store").as_deref(),
        Some(std::any::type_name::<RedisThrottler>()),
    );
    assert!(said.field("error").is_some(), "{:?}", said.fields);
    let others: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| {
            event.target == nest_rs_throttler::TARGET
                && event.message != FAILED
                && event.message != "rate-limit keying degraded to a shared bucket"
        })
        .collect();
    assert!(others.is_empty(), "the store said nothing: {others:#?}");
}
