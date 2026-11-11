//! A session's client holding its stream is sent MCP's `ping` once it is
//! silent, and its session closed when nothing comes back; a session holding
//! no stream is pinged by nothing.

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use nest_rs_core::module;
use nest_rs_http::StatusCode;
use nest_rs_mcp::McpConfig;
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{open_session, post_message};
use serde_json::{Value, json};

const PINGED: &str = "/mcp/pinged";

#[nest_rs_mcp::mcp(path = "/mcp/pinged")]
#[derive(Clone, Default)]
struct PingedTools;

#[nest_rs_mcp::tools]
impl PingedTools {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn echo(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("echo".to_owned())
    }
}

#[module(imports = [crate::open_mcp()], providers = [PingedTools])]
struct PingedModule;

const INTERVAL: Duration = Duration::from_secs(20);
const TIMEOUT: Duration = Duration::from_secs(20);

/// One place, so a session left open shows as a refused second one, an idle
/// timeout no test reaches, and the pings at their defaults.
async fn serving() -> TestApp {
    TestApp::builder()
        .provide(McpConfig {
            max_concurrent_sessions: 1,
            session_idle_timeout: Duration::from_secs(3600),
            // sse-stream re-arms its keep-alive on the std clock, which a paused
            // tokio clock would flood past.
            sse_keep_alive: None,
            ..McpConfig::default()
        })
        .module::<PingedModule>()
        .build()
        .await
        .expect("boots")
}

/// The session's standalone stream, as a client holding it reads it.
async fn hold_stream(app: &TestApp, session: &str) -> impl Stream<Item = Value> + Unpin {
    let response = app
        .http()
        .get(PINGED)
        .header("host", "localhost")
        .header("accept", "text/event-stream")
        .header("mcp-session-id", session)
        .send()
        .await;
    response.assert_status_is_ok();
    Box::pin(
        response
            .0
            .into_body()
            .into_stream()
            .map(|chunk| String::from_utf8_lossy(&chunk.expect("a chunk")).into_owned())
            .flat_map(|chunk| {
                futures_util::stream::iter(
                    chunk
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:"))
                        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
                        .collect::<Vec<_>>(),
                )
            }),
    )
}

/// Whether the session still serves its client.
async fn served(app: &TestApp, session: &str) -> bool {
    let answer = post_message(
        app.http(),
        PINGED,
        Some(session),
        None,
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list" }),
    )
    .await;
    answer.0.status() == StatusCode::OK
}

/// The client answers the ping `ping` on its session.
async fn answer(app: &TestApp, session: &str, ping: &Value) {
    assert_eq!(ping["method"], "ping", "{ping}");
    post_message(
        app.http(),
        PINGED,
        Some(session),
        None,
        &json!({ "jsonrpc": "2.0", "id": ping["id"], "result": {} }),
    )
    .await
    .assert_status(StatusCode::ACCEPTED);
}

/// A client that vanished holding its stream answers nothing: its session is
/// closed within the interval and the timeout of its last message, its place
/// freed and its stream ended, and one line says so, naming the mount.
#[tokio::test(start_paused = true)]
async fn a_client_answering_no_ping_loses_its_session_within_the_interval_and_the_timeout() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = serving().await;
    let session = open_session(app.http(), PINGED, None).await;
    let mut stream = hold_stream(&app, &session).await;

    let start = tokio::time::Instant::now();
    let ping = tokio::time::timeout(INTERVAL + Duration::from_secs(1), stream.next())
        .await
        .expect("pinged once silent for the interval")
        .expect("the stream is open");
    assert_eq!(ping["method"], "ping", "{ping}");
    assert!(
        ping["id"].is_string(),
        "an id no request of rmcp's takes: {ping}"
    );

    // The server's side keeps reading its stream, as the transport writing it
    // to a client that answers nothing does.
    let ended = tokio::time::timeout(TIMEOUT + Duration::from_secs(1), stream.next())
        .await
        .expect("the stream ends once the ping goes unanswered");
    assert!(ended.is_none(), "nothing more on the stream: {ended:?}");
    assert_eq!(start.elapsed(), INTERVAL + TIMEOUT);

    nest_rs_testing::wait_for(Duration::from_secs(1), || async {
        !served(&app, &session).await
    })
    .await;
    open_session(app.http(), PINGED, None).await;
    let closed = logs.expect_one(
        "nest_rs::mcp",
        "mcp session closed: the client sent nothing back within the ping timeout",
    );
    assert_eq!(closed.level, "warn");
    assert_eq!(closed.field("host").as_deref(), Some("McpModule"));
    assert_eq!(closed.field("path").as_deref(), Some(PINGED));
    assert_eq!(closed.field("ping_timeout_ms").as_deref(), Some("20000"));
    assert!(
        closed
            .fields
            .values()
            .all(|value| !value.contains(&session)),
        "the session id reaches no line: {:?}",
        closed.fields
    );
}

/// A client answering each ping keeps its session: pinged again an interval
/// after its answer, never given up.
#[tokio::test(start_paused = true)]
async fn a_client_answering_its_pings_keeps_its_session() {
    let app = serving().await;
    let session = open_session(app.http(), PINGED, None).await;
    let mut stream = hold_stream(&app, &session).await;

    let mut ids = Vec::new();
    for _ in 0..4 {
        let ping = stream.next().await.expect("pinged");
        answer(&app, &session, &ping).await;
        ids.push(ping["id"].clone());
    }

    assert!(served(&app, &session).await, "the session lives");
    let distinct: std::collections::HashSet<_> = ids.iter().map(Value::to_string).collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "each ping takes an id of its own: {ids:?}"
    );
}

/// Any message from the client is proof it is there: a client that talks is
/// never pinged.
#[tokio::test(start_paused = true)]
async fn a_client_that_talks_is_never_pinged() {
    let app = serving().await;
    let session = open_session(app.http(), PINGED, None).await;
    let mut stream = hold_stream(&app, &session).await;

    for _ in 0..6 {
        tokio::select! {
            biased;
            message = stream.next() => panic!("a talking client was sent {message:?}"),
            () = tokio::time::sleep(INTERVAL - Duration::from_secs(1)) => {
                assert!(served(&app, &session).await);
            }
        }
    }
}

/// A session holding no stream has no channel to ask through: nothing is
/// asked, and the session is not closed for the silence.
#[tokio::test(start_paused = true)]
async fn a_session_holding_no_stream_is_pinged_by_nothing_and_kept() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = serving().await;
    let session = open_session(app.http(), PINGED, None).await;

    tokio::time::sleep((INTERVAL + TIMEOUT) * 3).await;

    assert!(served(&app, &session).await, "the session lives");
    logs.expect_none(
        "nest_rs::mcp",
        "mcp session closed: the client sent nothing back within the ping timeout",
    );
}

/// With the pings off, a silent client holding its stream keeps its session.
#[tokio::test(start_paused = true)]
async fn an_interval_of_none_pings_no_one() {
    let app = TestApp::builder()
        .provide(McpConfig {
            session_idle_timeout: Duration::from_secs(3600),
            sse_keep_alive: None,
            ping_interval: None,
            ..McpConfig::default()
        })
        .module::<PingedModule>()
        .build()
        .await
        .expect("boots");
    let session = open_session(app.http(), PINGED, None).await;
    let mut stream = hold_stream(&app, &session).await;

    tokio::select! {
        biased;
        message = stream.next() => panic!("pinged with the pings off: {message:?}"),
        () = tokio::time::sleep((INTERVAL + TIMEOUT) * 3) => {}
    }
    assert!(served(&app, &session).await);
}

/// A ping and its answer are no use of the session: a client answering every
/// ping and sending nothing else idles out at the session's idle timeout.
#[tokio::test(start_paused = true)]
async fn a_client_answering_its_pings_still_idles_out() {
    const IDLE: Duration = Duration::from_secs(50);
    let app = TestApp::builder()
        .provide(McpConfig {
            max_concurrent_sessions: 1,
            session_idle_timeout: IDLE,
            sse_keep_alive: None,
            ..McpConfig::default()
        })
        .module::<PingedModule>()
        .build()
        .await
        .expect("boots");
    let session = open_session(app.http(), PINGED, None).await;
    let mut stream = hold_stream(&app, &session).await;

    let start = tokio::time::Instant::now();
    let mut answered = 0;
    while let Some(ping) = tokio::time::timeout(IDLE * 2, stream.next())
        .await
        .expect("the session idles out")
    {
        answer(&app, &session, &ping).await;
        answered += 1;
    }

    assert_eq!(answered, 2, "pinged at 20 s and 40 s, answered each");
    assert!(
        start.elapsed() <= IDLE + Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert!(!served(&app, &session).await, "the session idled out");
}
