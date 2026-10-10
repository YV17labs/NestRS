//! What a client is told when its request does not decode: the `400` problem's
//! `detail` names where and what kind, never the value, on every path an `Err`
//! is rendered by.

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

    /// A body the handler decodes itself, failing as a `400` through anyhow.
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

    /// A developer's own sentence in serde's vocabulary reaches the client as written.
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

#[controller(path = "/failure")]
struct FailureController;

#[routes]
impl FailureController {
    /// An error nobody meant for the client.
    #[get("/leaky")]
    async fn leaky(&self) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("secret 42"))
    }

    /// The same, wrapped by poem's own helper.
    #[get("/wrapped")]
    async fn wrapped(&self) -> poem::Result<String> {
        Err(poem::error::InternalServerError(std::io::Error::other(
            "secret 42",
        )))
    }

    /// A `Problem` an anyhow chain carries answers itself.
    #[get("/carried")]
    async fn carried(&self) -> anyhow::Result<String> {
        Err(anyhow::Error::from(nest_rs_core::Problem::new(
            404,
            nest_rs_core::problem::code::NOT_FOUND,
        ))
        .context("loading the post"))
    }

    /// A `Problem` returned through the edge's own error, crossed into poem.
    #[get("/taken")]
    async fn taken(&self) -> poem::Result<String> {
        let taken = nest_rs_core::Problem::new(409, nest_rs_core::problem::code::CONFLICT)
            .with_detail("the handle is taken")
            .with_retry_after(30);
        Err(nest_rs_http::__private::poem_bridge::error_to_poem(
            nest_rs_http::HttpError::from(taken),
        ))
    }

    /// An `anyhow` chain crossed into poem as an `HttpError`.
    #[get("/ledger")]
    async fn ledger(&self) -> poem::Result<String> {
        Err(nest_rs_http::__private::poem_bridge::error_to_poem(
            nest_rs_http::HttpError::from(anyhow::anyhow!("the ledger at 10.0.0.1 refused")),
        ))
    }

    /// A server `Problem` an anyhow chain carries, behind what the chain says
    /// of the failure.
    #[get("/outage")]
    async fn outage(&self) -> anyhow::Result<String> {
        Err(anyhow::Error::from(server_problem()).context("the search index at 10.0.0.1 refused"))
    }

    /// The same chain, crossed into poem as an `HttpError`.
    #[get("/outage-carried")]
    async fn outage_carried(&self) -> poem::Result<String> {
        Err(nest_rs_http::__private::poem_bridge::error_to_poem(
            nest_rs_http::HttpError::from(
                anyhow::Error::from(server_problem())
                    .context("the search index at 10.0.0.1 refused"),
            ),
        ))
    }

    /// A server `Problem` returned through the edge's own error.
    #[get("/unavailable")]
    async fn unavailable(&self) -> poem::Result<String> {
        Err(nest_rs_http::__private::poem_bridge::error_to_poem(
            nest_rs_http::HttpError::from(server_problem()),
        ))
    }

    /// A deliberate `500` with no cause: nothing withheld, nothing filed.
    #[get("/status")]
    async fn status(&self) -> poem::Result<String> {
        Err(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

fn server_problem() -> nest_rs_core::Problem {
    nest_rs_core::Problem::new(503, nest_rs_core::problem::code::UNAVAILABLE).with_retry_after(30)
}

#[module(providers = [FailureController])]
struct FailureModule;

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

async fn problem_json(resp: poem::test::TestResponse) -> serde_json::Value {
    resp.json().await.value().deserialize()
}

/// An error nobody meant for the client answers an opaque `500` problem and
/// files its whole chain once, at `error`, on every path an `Err` is rendered by.
#[tokio::test]
async fn an_error_no_one_meant_for_the_client_answers_an_opaque_500_and_files_its_chain() {
    for path in ["/failure/leaky", "/failure/wrapped"] {
        let logs = nest_rs_testing::LogCapture::install();
        let client = crate::boot::<FailureModule>().await;
        let resp = client.get(path).send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        resp.assert_content_type("application/problem+json");
        let body = problem_json(resp).await;
        assert!(!body.to_string().contains("secret"), "{path}: {body}");
        assert!(body.get("detail").is_none(), "{path}: {body}");

        let failed = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
        assert_eq!(failed.level, "error");
        assert!(
            failed
                .field("error")
                .is_some_and(|e| e.contains("secret 42")),
            "{path}: {failed:#?}",
        );
    }
}

/// A `Problem` in an anyhow chain, and one returned through `HttpError`, answer
/// their status, their detail, their `code` member and their wait.
#[tokio::test]
async fn a_problem_answers_its_status_and_its_code_member() {
    let client = crate::boot::<FailureModule>().await;

    let carried = client.get("/failure/carried").send().await;
    carried.assert_status(StatusCode::NOT_FOUND);
    let body = problem_json(carried).await;
    assert_eq!(body["code"], "NOT_FOUND", "{body}");

    let taken = client.get("/failure/taken").send().await;
    taken.assert_status(StatusCode::CONFLICT);
    taken.assert_header(poem::http::header::RETRY_AFTER, "30");
    let body = problem_json(taken).await;
    assert_eq!(body["code"], "CONFLICT", "{body}");
    assert_eq!(body["detail"], "the handle is taken", "{body}");
}

/// A server `Problem` answers its status, its code and its wait, never the
/// chain behind it, which every path that renders it files once at `error`;
/// a bare `Problem`, like a bare status, withholds nothing and files nothing,
/// and so does a client one.
#[tokio::test]
async fn a_server_problem_answers_its_status_and_files_the_chain_it_withholds() {
    for path in ["/failure/outage", "/failure/outage-carried"] {
        let logs = nest_rs_testing::LogCapture::install();
        let client = crate::boot::<FailureModule>().await;
        let resp = client.get(path).send().await;
        resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        resp.assert_header(poem::http::header::RETRY_AFTER, "30");
        let body = problem_json(resp).await;
        assert_eq!(body["code"], "UNAVAILABLE", "{path}: {body}");
        assert!(!body.to_string().contains("10.0.0.1"), "{path}: {body}");

        let failed = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
        assert_eq!(failed.level, "error", "{path}");
        assert!(
            failed
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1") && e.contains("503 UNAVAILABLE")),
            "{path}: {failed:#?}",
        );
    }

    let logs = nest_rs_testing::LogCapture::install();
    let client = crate::boot::<FailureModule>().await;
    let bare = client.get("/failure/unavailable").send().await;
    bare.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    bare.assert_header(poem::http::header::RETRY_AFTER, "30");
    assert_eq!(problem_json(bare).await["code"], "UNAVAILABLE");
    for path in ["/failure/carried", "/failure/taken"] {
        let resp = client.get(path).send().await;
        assert!(resp.0.status().is_client_error(), "{path}");
    }
    logs.expect_none(nest_rs_http::target::HTTP, "request failed");
}

/// An `HttpError` whose chain the client is owed no word of answers opaquely
/// and files it.
#[tokio::test]
async fn an_http_error_withholding_its_chain_answers_opaquely_and_files_it() {
    let logs = nest_rs_testing::LogCapture::install();
    let client = crate::boot::<FailureModule>().await;
    let resp = client.get("/failure/ledger").send().await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let body = problem_json(resp).await;
    assert!(!body.to_string().contains("10.0.0.1"), "{body}");
    let failed = logs.expect_one(nest_rs_http::target::HTTP, "request failed");
    assert!(
        failed
            .field("error")
            .is_some_and(|e| e.contains("10.0.0.1")),
        "{failed:#?}"
    );
}

/// A bare status the handler chose withholds nothing, so nothing is filed.
#[tokio::test]
async fn a_bare_status_is_answered_and_files_nothing() {
    let logs = nest_rs_testing::LogCapture::install();
    let client = crate::boot::<FailureModule>().await;
    client
        .get("/failure/status")
        .send()
        .await
        .assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        logs.find(nest_rs_http::target::HTTP, "request failed")
            .is_empty(),
        "{:#?}",
        logs.events(),
    );
}
