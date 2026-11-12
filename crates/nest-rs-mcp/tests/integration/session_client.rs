//! A session is in use while its client and it exchange messages — any the
//! client sends but a ping's answer, any of the server's a stream carries to it —
//! or while a call waits on its answer; past the idle timeout of that, it closes.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use nest_rs_mcp::McpConfig;
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{open_session, post_message, result};

use crate::{
    WAIT, WRITING, answer, call, events, hold_stream, open_stream, opens_a_session, served, writing,
};

const IDLE: Duration = Duration::from_secs(50);

/// One place, so a session left open shows as a refused second one.
fn one_place() -> McpConfig {
    McpConfig {
        max_concurrent_sessions: 1,
        session_idle_timeout: IDLE,
        ..McpConfig::default()
    }
}

/// What the server writes to a session holding no stream reaches no client: the
/// session idles out at the idle timeout of its client's last message, its place
/// freed, while the server keeps writing.
#[tokio::test(start_paused = true)]
async fn a_session_the_server_writes_to_with_no_stream_idles_out() {
    let app = writing(one_place()).await;
    let session = open_session(app.http(), WRITING, None).await;
    call(&app, WRITING, &session, "tick").await;

    tokio::time::sleep(IDLE - Duration::from_secs(1)).await;
    assert!(
        !opens_a_session(&app, WRITING).await,
        "held until the idle timeout"
    );
    nest_rs_testing::wait_for(Duration::from_secs(2), || opens_a_session(&app, WRITING)).await;

    assert!(
        !served(&app, WRITING, &session).await,
        "the session idled out"
    );
}

/// A client listening on its stream is carried what the server writes: each
/// message is use of the session, which outlives its idle timeout while they
/// flow and its client answers its pings.
#[tokio::test(start_paused = true)]
async fn a_client_listening_while_the_server_writes_keeps_its_session() {
    let app = writing(one_place()).await;
    let session = open_session(app.http(), WRITING, None).await;
    let mut stream = hold_stream(&app, WRITING, &session).await;
    call(&app, WRITING, &session, "tick").await;

    let start = tokio::time::Instant::now();
    let mut carried = 0;
    while start.elapsed() < IDLE * 3 {
        let message = tokio::time::timeout(IDLE, stream.next())
            .await
            .expect("a message within the idle timeout")
            .expect("the stream is open");
        if message["method"] == "ping" {
            answer(&app, WRITING, &session, &message).await;
        } else {
            carried += 1;
        }
    }

    assert!(carried >= 14, "a notification every tick: {carried}");
    assert!(served(&app, WRITING, &session).await, "the session lives");
}

/// A call is use of its session until it is answered: one outlasting the idle
/// timeout gets its answer, and the session serves on.
#[tokio::test(start_paused = true)]
async fn a_call_outlasting_the_idle_timeout_is_answered() {
    let app = writing(McpConfig {
        operation_timeout: None,
        ..one_place()
    })
    .await;
    let session = open_session(app.http(), WRITING, None).await;

    let start = tokio::time::Instant::now();
    let answered = call(&app, WRITING, &session, "wait").await;

    assert_eq!(start.elapsed(), WAIT);
    assert_eq!(
        result(&answered)["result"]["content"][0]["text"],
        "wait",
        "{answered}"
    );
    assert!(served(&app, WRITING, &session).await, "the session lives");
}

/// Call `tool` on `session` as a client whose connection breaks once it read the
/// call's first event: the id that event carries, which resumes the call's
/// stream.
async fn call_and_break(app: &TestApp, session: &str, tool: &str) -> String {
    let posted = post_message(
        app.http(),
        WRITING,
        Some(session),
        None,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": { "name": tool, "arguments": {} },
        }),
    )
    .await;
    posted.assert_status_is_ok();
    let mut body = posted.0.into_body().into_stream();
    let mut read = String::new();
    loop {
        let chunk = body
            .next()
            .await
            .expect("the call's stream is open")
            .expect("a chunk");
        read.push_str(&String::from_utf8_lossy(&chunk));
        if let Some(id) = read.lines().find_map(|line| line.strip_prefix("id:")) {
            return id.trim().to_owned();
        }
    }
}

/// A call waiting on its answer on a stream its client resumed is use of its
/// session as on its first: one outlasting the idle timeout, its connection
/// broken and resumed, is answered, and the session serves on.
#[tokio::test(start_paused = true)]
async fn a_call_resumed_and_outlasting_the_idle_timeout_is_answered() {
    let app = writing(McpConfig {
        operation_timeout: None,
        ..one_place()
    })
    .await;
    let session = open_session(app.http(), WRITING, None).await;

    let start = tokio::time::Instant::now();
    let last_event = call_and_break(&app, &session, "wait").await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut resumed = events(open_stream(&app, WRITING, &session, Some(&last_event)).await);
    let answered = loop {
        let message = resumed
            .next()
            .await
            .expect("the resumed stream carries the answer");
        if message["method"] == "ping" {
            answer(&app, WRITING, &session, &message).await;
        } else {
            break message;
        }
    };

    assert_eq!(start.elapsed(), WAIT);
    assert_eq!(
        answered["result"]["content"][0]["text"], "wait",
        "{answered}"
    );
    assert!(served(&app, WRITING, &session).await, "the session lives");
}

/// A session given up with a call in flight ends for its client at once: its
/// requests read `404`, as one the mount does not hold, and nothing is filed as
/// failed; rmcp gives that call five seconds, then cancels it, and the place
/// frees.
#[tokio::test(start_paused = true)]
async fn a_session_given_up_with_a_call_in_flight_reads_as_one_the_mount_does_not_hold() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = Arc::new(
        writing(McpConfig {
            operation_timeout: None,
            session_idle_timeout: Duration::from_secs(3600),
            ..one_place()
        })
        .await,
    );
    let session = open_session(app.http(), WRITING, None).await;
    let mut listening = hold_stream(&app, WRITING, &session).await;
    let in_flight = tokio::spawn({
        let app = Arc::clone(&app);
        let session = session.clone();
        async move { call(&app, WRITING, &session, "wait").await }
    });
    while listening.next().await.is_some() {}

    let refused = post_message(
        app.http(),
        WRITING,
        Some(&session),
        None,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list" }),
    )
    .await;
    refused.assert_status(nest_rs_http::StatusCode::NOT_FOUND);
    logs.expect_none("nest_rs::mcp", "mcp session manager failed");

    assert!(
        !opens_a_session(&app, WRITING).await,
        "held while rmcp waits on the call"
    );
    nest_rs_testing::wait_for(Duration::from_secs(6), || opens_a_session(&app, WRITING)).await;
    let cancelled = logs
        .find("nest_rs::operation", "mcp.operation")
        .into_iter()
        .filter(|line| line.field("outcome").as_deref() == Some("cancelled"))
        .count();
    assert_eq!(cancelled, 1, "the call is cancelled");
    in_flight.await.expect("the call's stream ends");
}
