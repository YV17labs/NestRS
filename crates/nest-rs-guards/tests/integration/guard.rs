//! A guard that reads the request body: a webhook signed per Standard Webhooks
//! (`webhook-id`, `webhook-timestamp`, `webhook-signature`) is verified in a
//! [`Guard`] before its handler, which reads the same bytes after it. The read
//! is the edge's like any other: under the transport's cap and its gap bound.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::hmac;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nest_rs_core::{Layer, UnitContext, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::{HttpConfig, HttpModule, HttpView, RawBody, async_trait, controller, routes};
use nest_rs_testing::TestApp;

/// The secret the sender signs with.
const SECRET: &[u8] = b"a webhook secret";

/// How far a timestamp may sit from now, either way: Standard Webhooks' five
/// minutes.
const TOLERANCE_SECS: u64 = 5 * 60;

const CAP: usize = 256;

/// Seconds since the epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs()
}

/// The `v1` signature of `body` sent as message `id` at `timestamp`.
fn sign(key: &[u8], id: &str, timestamp: u64, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let signed = [
        id.as_bytes(),
        b".",
        timestamp.to_string().as_bytes(),
        b".",
        body,
    ]
    .concat();
    format!("v1,{}", STANDARD.encode(hmac::sign(&key, &signed)))
}

/// Admits a request whose body carries a valid signature from [`SECRET`], sent
/// within [`TOLERANCE_SECS`], and hands the handler the bytes it verified.
#[injectable]
#[derive(Default)]
struct WebhookSignature;

impl Layer for WebhookSignature {}

#[async_trait]
impl Guard for WebhookSignature {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let req = HttpView::of(cx)?.request_mut();
        let header = |name| req.headers().get(name).and_then(|v| v.to_str().ok());
        let (Some(id), Some(timestamp), Some(signatures)) = (
            header("webhook-id"),
            header("webhook-timestamp").and_then(|t| t.parse::<u64>().ok()),
            header("webhook-signature"),
        ) else {
            return Err(Denial::unauthorized("an unsigned webhook"));
        };
        if now().abs_diff(timestamp) > TOLERANCE_SECS {
            return Err(Denial::unauthorized("a webhook outside the replay window"));
        }
        let id = id.to_owned();
        let signatures: Vec<Vec<u8>> = signatures
            .split(' ')
            .filter_map(|s| s.strip_prefix("v1,"))
            .filter_map(|s| STANDARD.decode(s).ok())
            .collect();

        // The edge's read: its cap and its gap bound the body, and answer a
        // read they stop themselves.
        let body = req.take_body().into_bytes().await.map_err(|error| {
            Denial::unauthorized(format!("an unreadable webhook body: {error}"))
        })?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, SECRET);
        let signed = [
            id.as_bytes(),
            b".",
            timestamp.to_string().as_bytes(),
            b".",
            &body,
        ]
        .concat();
        if !signatures
            .iter()
            .any(|tag| hmac::verify(&key, &signed, tag).is_ok())
        {
            return Err(Denial::unauthorized(
                "a webhook signature that does not verify",
            ));
        }
        req.set_body(body);
        Ok(())
    }
}

impl GuardFor<HttpView> for WebhookSignature {}

#[controller(path = "/webhooks")]
struct WebhooksController;

#[routes]
impl WebhooksController {
    /// Answers the bytes it was handed.
    #[post("/orders")]
    #[use_guards(WebhookSignature)]
    #[public]
    async fn orders(&self, body: RawBody) -> Vec<u8> {
        body.into_inner().to_vec()
    }
}

#[module(
    imports = [HttpModule::for_root(HttpConfig { max_body_bytes: Some(CAP), ..Default::default() })],
    providers = [WebhookSignature, WebhooksController],
)]
struct WebhooksModule;

async fn app() -> TestApp {
    TestApp::for_module::<WebhooksModule>()
        .await
        .expect("a route binding a body-reading guard boots")
}

#[tokio::test]
async fn a_guard_that_reads_the_body_sets_it_back_for_the_handler() {
    let body = br#"{"order":"1042","status":"paid"}"#;
    let sent_at = now();
    let resp = app()
        .await
        .http()
        .post("/webhooks/orders")
        .header("webhook-id", "msg_1")
        .header("webhook-timestamp", sent_at.to_string())
        .header("webhook-signature", sign(SECRET, "msg_1", sent_at, body))
        .body(body.to_vec())
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_bytes(body.to_vec()).await;
}

#[tokio::test]
async fn a_body_signed_with_another_key_never_reaches_the_handler() {
    let body = br#"{"order":"1042","status":"paid"}"#;
    let sent_at = now();
    let resp = app()
        .await
        .http()
        .post("/webhooks/orders")
        .header("webhook-id", "msg_1")
        .header("webhook-timestamp", sent_at.to_string())
        .header(
            "webhook-signature",
            sign(b"another key", "msg_1", sent_at, body),
        )
        .body(body.to_vec())
        .send()
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_signature_replayed_past_the_window_is_refused() {
    let body = br#"{"order":"1042","status":"paid"}"#;
    let sent_at = now() - TOLERANCE_SECS - 60;
    let resp = app()
        .await
        .http()
        .post("/webhooks/orders")
        .header("webhook-id", "msg_1")
        .header("webhook-timestamp", sent_at.to_string())
        .header("webhook-signature", sign(SECRET, "msg_1", sent_at, body))
        .body(body.to_vec())
        .send()
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

/// The guard's read is the transport's: past the cap it answers `413`, whatever
/// the guard made of the failed read.
#[tokio::test]
async fn past_the_cap_the_guards_read_answers_413() {
    let body = vec![b'x'; CAP + 1];
    let sent_at = now();
    let resp = app()
        .await
        .http()
        .post("/webhooks/orders")
        .header("webhook-id", "msg_1")
        .header("webhook-timestamp", sent_at.to_string())
        .header("webhook-signature", sign(SECRET, "msg_1", sent_at, &body))
        .body(body)
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}
