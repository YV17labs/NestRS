//! What a client is told when its request does not decode: the `400` problem's
//! `detail` names where and what kind, never the value — on every deserialising
//! extractor, on a handler's own decode, and on both paths an `Err` is rendered
//! by (the edge's own tail, and the outer wrap CORS or compression mounts).
//!
//! poem's extractors answer `parse error: <serde's sentence>`, and serde's
//! sentence quotes what the client sent; the normalizer passed it through as the
//! detail. A body field is where a card number or a password travels, and a
//! `400` is logged by proxies and kept by caches.

use nest_rs_core::{App, Layer, Transport, injectable, module};
use nest_rs_http::{
    HttpTransport, Opaque, ProblemDetails, Valid, async_trait, controller, input, routes,
};
use nest_rs_interceptors::{AppBuilderInterceptorsExt, Interceptor, Next, interceptor};
use poem::http::StatusCode;
use poem::test::TestClient;
use poem::web::{Form, Json, Query};
use serde::{Deserialize, Serialize};

const SECRET: &str = "sk_live_51HsecretTOKEN";

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
enum CardKind {
    Visa,
}

#[input]
struct Charge {
    #[validate(range(min = 1))]
    amount: u64,
}

#[input]
struct Filter {
    card: CardKind,
}

#[controller(path = "/decode")]
struct DecodeController;

#[routes]
impl DecodeController {
    #[post("/json")]
    async fn json(&self, body: Json<Charge>) -> String {
        body.0.amount.to_string()
    }

    #[post("/valid")]
    async fn valid(&self, body: Valid<Json<Charge>>) -> String {
        body.amount.to_string()
    }

    #[get("/query")]
    async fn query(&self, filter: Query<Filter>) -> String {
        format!("{:?}", filter.0.card)
    }

    #[post("/form")]
    async fn form(&self, filter: Form<Filter>) -> String {
        format!("{:?}", filter.0.card)
    }

    /// A body the handler decodes itself, failing as a `400` through anyhow —
    /// the error is the handler's, the reply is still the edge's.
    #[post("/own")]
    async fn own(&self, body: String) -> poem::Result<String> {
        let amount: u64 = serde_json::from_str(&body).map_err(|err| {
            poem::Error::from((StatusCode::BAD_REQUEST, anyhow::Error::from(err)))
        })?;
        Ok(amount.to_string())
    }

    /// The same decode, mapped the way poem's docs map one.
    #[post("/bad-request")]
    async fn bad_request(&self, body: String) -> poem::Result<String> {
        let amount: u64 = serde_json::from_str(&body).map_err(poem::error::BadRequest)?;
        Ok(amount.to_string())
    }

    /// A problem built from the error by the framework's own constructor.
    #[post("/problem")]
    async fn problem(&self, body: String) -> Result<String, ProblemDetails> {
        let amount: u64 = serde_json::from_str(&body).map_err(|err| {
            ProblemDetails::from_error(StatusCode::BAD_REQUEST, "Bad Request", &err)
        })?;
        Ok(amount.to_string())
    }

    /// A developer's own sentence in serde's vocabulary is theirs, and reaches
    /// the client as written.
    #[get("/own-words")]
    async fn own_words(&self) -> poem::Result<String> {
        Err(poem::Error::from_string(
            "invalid value: must be positive",
            StatusCode::BAD_REQUEST,
        ))
    }

    /// A failure the client must not read, carrying a decode failure in an
    /// anyhow chain: the operator's line says it without the value.
    #[post("/opaque")]
    async fn opaque(&self, body: String) -> poem::Result<String> {
        let amount: u64 = serde_json::from_str(&body)
            .map_err(anyhow::Error::from)
            .opaque()?;
        Ok(amount.to_string())
    }
}

#[module(providers = [DecodeController, Forwarding])]
struct DecodeModule;

type Client = TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>>;

/// An interceptor that only forwards: registering any wrap moves the rendering
/// of a still-unhandled `Err` to the `ERROR_RESOLVE` band, below every wrap.
#[injectable]
#[derive(Default)]
struct Forwarding;

impl Layer for Forwarding {}

#[async_trait]
impl Interceptor for Forwarding {
    async fn intercept(&self, req: poem::Request, next: Next<'_>) -> poem::Result<poem::Response> {
        next.run(req).await
    }
}

/// The app every real deployment is: one with a wrap registered.
async fn wrapped() -> Client {
    let app = App::builder()
        .use_interceptors_global([interceptor::<Forwarding>()])
        .module::<DecodeModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new();
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    TestClient::new(
        transport
            .take_endpoint()
            .expect("configure populates the endpoint"),
    )
}

/// Every place an `Err` is rendered: the fused edge, the outer wrap compression
/// mounts, and the `ERROR_RESOLVE` band a registered wrap moves it to.
async fn clients() -> [Client; 3] {
    [
        crate::boot::<DecodeModule>().await,
        crate::boot_on::<DecodeModule>(HttpTransport::new().compression(true)).await,
        wrapped().await,
    ]
}

async fn detail(resp: poem::test::TestResponse) -> String {
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.value().deserialize();
    body["detail"].as_str().unwrap_or_default().to_owned()
}

fn secret_json() -> String {
    format!(r#"{{"amount":"{SECRET}"}}"#)
}

/// Every event field the capture holds that spells `needle`.
fn quoting(logs: &nest_rs_testing::LogCapture, needle: &str) -> Vec<String> {
    logs.events()
        .into_iter()
        .filter(|event| event.fields.values().any(|value| value.contains(needle)))
        .map(|event| format!("{} {:?}", event.message, event.fields))
        .collect()
}

#[tokio::test]
async fn a_body_that_does_not_decode_is_refused_without_its_value() {
    let logs = nest_rs_testing::LogCapture::install();
    for client in clients().await {
        for path in ["/decode/json", "/decode/valid"] {
            let resp = client
                .post(path)
                .content_type("application/json")
                .body(secret_json())
                .send()
                .await;
            assert_eq!(
                detail(resp).await,
                "parse error: invalid type: a string, expected u64 at line 1 column 34",
                "{path}",
            );
        }
        // A kind serde names without quoting is read off the chain: only the
        // typed reading says it as the report does.
        for path in ["/decode/json", "/decode/valid"] {
            let resp = client
                .post(path)
                .content_type("application/json")
                .body(r#"{"amount":["x"]}"#)
                .send()
                .await;
            assert_eq!(
                detail(resp).await,
                "parse error: invalid type: a sequence, expected u64 at line 1 column 10",
                "{path}",
            );
        }
        // A key the client spelled is the client's too.
        let resp = client
            .post("/decode/json")
            .content_type("application/json")
            .body(format!(r#"{{"amount":1,"{SECRET}":1}}"#))
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "parse error: unknown field, expected `amount` at line 1 column 36",
        );
    }
    assert!(
        quoting(&logs, SECRET).is_empty(),
        "{:#?}",
        quoting(&logs, SECRET)
    );
}

#[tokio::test]
async fn a_query_or_a_form_that_does_not_decode_is_refused_without_its_value() {
    for client in clients().await {
        let resp = client
            .get("/decode/query")
            .query("card", &SECRET)
            .send()
            .await;
        assert_eq!(detail(resp).await, "unknown variant, expected `Visa`");
        let resp = client
            .post("/decode/form")
            .content_type("application/x-www-form-urlencoded")
            .body(format!("card={SECRET}"))
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "url decode: unknown variant, expected `Visa`"
        );
    }
}

#[tokio::test]
async fn a_handler_s_own_decode_failure_is_refused_without_its_value() {
    let body = format!(r#""{SECRET}""#);
    for client in clients().await {
        for path in ["/decode/own", "/decode/bad-request", "/decode/problem"] {
            let resp = client.post(path).body(body.clone()).send().await;
            assert_eq!(
                detail(resp).await,
                "invalid type: a string, expected u64 at line 1 column 24",
                "{path}",
            );
        }
        let resp = client.get("/decode/own-words").send().await;
        assert_eq!(detail(resp).await, "invalid value: must be positive");
    }
}

/// The operator's half: `.opaque()` over an anyhow-wrapped decode failure logs
/// the report, at `error`, while the client reads the constant.
#[tokio::test]
async fn an_opaque_decode_failure_is_logged_without_its_value() {
    let logs = nest_rs_testing::LogCapture::install();
    let client = crate::boot::<DecodeModule>().await;
    let resp = client
        .post("/decode/opaque")
        .body(format!(r#""{SECRET}""#))
        .send()
        .await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let line = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
    assert_eq!(
        line.field("error").as_deref(),
        Some("invalid type: a string, expected u64 at line 1 column 24"),
    );
    assert!(
        quoting(&logs, SECRET).is_empty(),
        "{:#?}",
        quoting(&logs, SECRET)
    );
}
