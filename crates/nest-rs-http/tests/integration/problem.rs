//! What a client is told when its request does not decode — the `400` problem's
//! `detail` names where and what kind, never the value — and when the server
//! fails without saying why — one document, its class's `code` and no `detail` —
//! on every path an `Err` is rendered by.

use nest_rs_core::{
    App, GlobalInterceptors, Interceptor, Layer, Module, Next, Outcome, Problem, ToProblem,
    Transport, UnitContext, injectable, module,
};
use nest_rs_core::{async_trait, code, input};
use nest_rs_http::StatusCode;
use nest_rs_http::{
    Form, HttpConfig, HttpError, Json, Opaque, ProblemDetails, Query, RawBody, ResponseError,
    Result, SseEvent, SseStream, controller, futures_util::stream, routes,
};
use nest_rs_testing::TestClient;
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
    #[public]
    async fn json(&self, body: Json<Charge>) -> String {
        body.0.amount.to_string()
    }

    #[get("/query")]
    #[public]
    async fn query(&self, filter: Query<Filter>) -> String {
        format!("{:?}", filter.0.card)
    }

    #[post("/form")]
    #[public]
    async fn form(&self, filter: Form<Filter>) -> String {
        format!("{:?}", filter.0.card)
    }

    /// A body the handler decodes itself, failing as a `400` it names.
    #[post("/own")]
    #[public]
    async fn own(&self, body: RawBody) -> Result<String> {
        let amount: u64 = serde_json::from_slice(&body)
            .map_err(|err| HttpError::new(err, StatusCode::BAD_REQUEST))?;
        Ok(amount.to_string())
    }

    /// The same decode, through an error of the handler's own that knows its status.
    #[post("/bad-request")]
    #[public]
    async fn bad_request(&self, body: RawBody) -> std::result::Result<String, BadAmount> {
        let amount: u64 = serde_json::from_slice(&body)?;
        Ok(amount.to_string())
    }

    /// A problem built from the error by the framework's own constructor.
    #[post("/problem")]
    #[public]
    async fn problem(&self, body: RawBody) -> std::result::Result<String, ProblemDetails> {
        let amount: u64 = serde_json::from_slice(&body).map_err(|err| {
            ProblemDetails::from_error(StatusCode::BAD_REQUEST, "Bad Request", &err)
        })?;
        Ok(amount.to_string())
    }

    /// A developer's own sentence in serde's vocabulary reaches the client as written.
    #[get("/own-words")]
    #[public]
    async fn own_words(&self) -> Result<String> {
        Err(HttpError::new(MustBePositive, StatusCode::BAD_REQUEST))
    }

    /// A failure the client must not read, carrying a decode failure in an
    /// anyhow chain: the operator's line says it without the value.
    #[post("/opaque")]
    #[public]
    async fn opaque(&self, body: RawBody) -> Result<String> {
        let amount: u64 = serde_json::from_slice(&body)
            .map_err(anyhow::Error::from)
            .opaque()?;
        Ok(amount.to_string())
    }
}

/// A handler's own decode failure, which knows its status.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct BadAmount(#[from] serde_json::Error);

impl ResponseError for BadAmount {
    fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid value: must be positive")]
struct MustBePositive;

#[module(providers = [DecodeController, Forwarding])]
struct DecodeModule;

/// What the search index's failure says of itself, its problem beneath.
#[derive(Debug, thiserror::Error)]
#[error("the search index at 10.0.0.1 refused")]
struct IndexRefused(#[source] nest_rs_core::Problem);

#[controller(path = "/failure")]
struct FailureController;

#[routes]
impl FailureController {
    /// An error nobody meant for the client.
    #[get("/leaky")]
    #[public]
    async fn leaky(&self) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("secret 42"))
    }

    /// The same, wrapped with its status.
    #[get("/wrapped")]
    #[public]
    async fn wrapped(&self) -> Result<String> {
        Err(HttpError::new(
            std::io::Error::other("secret 42"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }

    /// A `Problem` an anyhow chain carries answers itself.
    #[get("/carried")]
    #[public]
    async fn carried(&self) -> anyhow::Result<String> {
        Err(anyhow::Error::from(nest_rs_core::Problem::new(
            404,
            nest_rs_core::code::NOT_FOUND,
        ))
        .context("loading the post"))
    }

    /// A `Problem` returned through the edge's own error.
    #[get("/taken")]
    #[public]
    async fn taken(&self) -> Result<String> {
        let taken = nest_rs_core::Problem::new(409, nest_rs_core::code::CONFLICT)
            .with_detail("the handle is taken")
            .with_retry_after(30);
        Err(HttpError::from(taken))
    }

    /// An `anyhow` chain said opaque where the body answers an `HttpError`.
    #[get("/ledger")]
    #[public]
    async fn ledger(&self) -> Result<String> {
        let refused: anyhow::Result<String> =
            Err(anyhow::anyhow!("the ledger at 10.0.0.1 refused"));
        refused.opaque()
    }

    /// A server `Problem` an anyhow chain carries, behind what the chain says
    /// of the failure.
    #[get("/outage")]
    #[public]
    async fn outage(&self) -> anyhow::Result<String> {
        Err(anyhow::Error::from(server_problem()).context("the search index at 10.0.0.1 refused"))
    }

    /// The same server `Problem`, its chain kept where the body answers an
    /// `HttpError`.
    #[get("/outage-carried")]
    #[public]
    async fn outage_carried(&self) -> Result<String> {
        Err(HttpError::new(
            IndexRefused(server_problem()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }

    /// A server `Problem` returned through the edge's own error.
    #[get("/unavailable")]
    #[public]
    async fn unavailable(&self) -> Result<String> {
        Err(HttpError::from(server_problem()))
    }

    /// A deliberate `500` with no cause: nothing withheld, nothing filed.
    #[get("/status")]
    #[public]
    async fn status(&self) -> Result<String> {
        Err(HttpError::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }

    /// A failure the handler says is none of the client's business.
    #[get("/opaque")]
    #[public]
    async fn opaque(&self) -> Result<String> {
        Err::<String, _>(std::io::Error::other("secret 42")).opaque()
    }

    /// A `500` the handler wrote as raw text.
    #[get("/raw")]
    #[public]
    async fn raw(&self) -> nest_rs_http::Response {
        nest_rs_http::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body("secret 42")
    }

    /// A handler that unwinds.
    #[get("/panic")]
    #[public]
    async fn panic(&self) -> String {
        panic!("secret 42")
    }

    /// An error of the handler's own whose status is a `500`, rendered by
    /// `ResponseError`'s default.
    #[get("/typed")]
    #[public]
    async fn typed(&self) -> std::result::Result<String, LedgerDown> {
        Err(LedgerDown)
    }

    /// The same, over a decode failure quoting what the request sent.
    #[post("/typed-decode")]
    #[public]
    async fn typed_decode(&self, body: RawBody) -> std::result::Result<String, LedgerUnreadable> {
        let amount: u64 = serde_json::from_slice(&body)?;
        Ok(amount.to_string())
    }

    /// An error of the handler's own whose status is a `503`.
    #[get("/typed-busy")]
    #[public]
    async fn typed_busy(&self) -> std::result::Result<String, LedgerBusy> {
        Err(LedgerBusy(std::io::Error::other(
            "the ledger at 10.0.0.1 timed out",
        )))
    }

    /// An error of the handler's own whose status is a `4xx`.
    #[get("/typed-refusal")]
    #[public]
    async fn typed_refusal(&self) -> std::result::Result<String, Overdrawn> {
        Err(Overdrawn)
    }

    /// The problem document itself returned as the error.
    #[get("/document")]
    #[public]
    async fn document(&self) -> std::result::Result<String, ProblemDetails> {
        Err(ProblemDetails::internal())
    }

    /// The class's own problem, as `#[crud]` answers an opaque failure.
    #[get("/problem")]
    #[public]
    async fn problem(&self) -> Result<String> {
        Err(HttpError::from(nest_rs_core::Problem::new(
            500,
            nest_rs_core::code::INTERNAL,
        )))
    }

    /// A bare `503`, the server unable to serve the request.
    #[get("/busy")]
    #[public]
    async fn busy(&self) -> Result<String> {
        Err(HttpError::from_status(StatusCode::SERVICE_UNAVAILABLE))
    }

    /// A bare `404`, the handler's own.
    #[get("/missing")]
    #[public]
    async fn missing(&self) -> Result<String> {
        Err(HttpError::from_status(StatusCode::NOT_FOUND))
    }

    /// A `404` the handler wrote as raw text.
    #[get("/raw-missing")]
    #[public]
    async fn raw_missing(&self) -> nest_rs_http::Response {
        nest_rs_http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("no row 42")
    }
}

/// A failure whose sentence names what the client must not read.
#[derive(Debug, thiserror::Error)]
#[error("the ledger at 10.0.0.1 holds secret 42")]
struct LedgerDown;

impl ResponseError for LedgerDown {
    fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// A `500` of the handler's own over a decode failure.
#[derive(Debug, thiserror::Error)]
#[error("the ledger entry does not decode")]
struct LedgerUnreadable(#[from] serde_json::Error);

impl ResponseError for LedgerUnreadable {
    fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// A `503` of the handler's own, over what it could not reach.
#[derive(Debug, thiserror::Error)]
#[error("the ledger is busy")]
struct LedgerBusy(#[source] std::io::Error);

impl ResponseError for LedgerBusy {
    fn status(&self) -> StatusCode {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// A refusal of the handler's own.
#[derive(Debug, thiserror::Error)]
#[error("the account is overdrawn")]
struct Overdrawn;

impl ResponseError for Overdrawn {
    fn status(&self) -> StatusCode {
        StatusCode::CONFLICT
    }
}

fn server_problem() -> nest_rs_core::Problem {
    nest_rs_core::Problem::new(503, nest_rs_core::code::UNAVAILABLE).with_retry_after(30)
}

#[module(providers = [FailureController, Forwarding])]
struct FailureModule;

const OVERDRAWN: Problem = Problem::new(409, code::CONFLICT).with_type(
    "https://example.com/problems/overdrawn",
    "Account overdrawn",
);

/// A domain error mapped to the wire once, through `ToProblem` alone.
#[derive(Debug, thiserror::Error)]
enum LedgerError {
    #[error("account 7 is overdrawn")]
    Overdrawn,
    #[error("the transfer is refused")]
    Refused,
    #[error("the ledger is busy")]
    Busy(#[source] std::io::Error),
    #[error("the ledger at 10.0.0.1 holds secret 42")]
    Down,
}

impl ToProblem for LedgerError {
    fn to_problem(&self) -> Option<Problem> {
        match self {
            Self::Overdrawn => Some(OVERDRAWN.with_detail(self.to_string())),
            Self::Refused => Some(
                Problem::new(422, code::INVALID_ARGUMENT)
                    .with_detail(self.to_string())
                    .with_errors(serde_json::json!({ "amount": [{ "code": "range" }] })),
            ),
            Self::Busy(_) => Some(Problem::new(503, code::UNAVAILABLE).with_retry_after(30)),
            Self::Down => None,
        }
    }
}

#[controller(path = "/domain")]
struct DomainController;

#[routes]
impl DomainController {
    #[get("/overdrawn")]
    #[public]
    async fn overdrawn(&self) -> std::result::Result<String, LedgerError> {
        Err(LedgerError::Overdrawn)
    }

    /// The same error, `?`'d where the body answers the edge's own error.
    #[get("/converted")]
    #[public]
    async fn converted(&self) -> Result<String> {
        let overdrawn: std::result::Result<String, LedgerError> = Err(LedgerError::Overdrawn);
        Ok(overdrawn?)
    }

    #[get("/refused")]
    #[public]
    async fn refused(&self) -> std::result::Result<String, LedgerError> {
        Err(LedgerError::Refused)
    }

    #[get("/busy")]
    #[public]
    async fn busy(&self) -> std::result::Result<String, LedgerError> {
        Err(LedgerError::Busy(std::io::Error::other(
            "the ledger at 10.0.0.1 timed out",
        )))
    }

    #[get("/down")]
    #[public]
    async fn down(&self) -> std::result::Result<String, LedgerError> {
        Err(LedgerError::Down)
    }

    #[post("/transfer")]
    #[public]
    #[http_code(201)]
    async fn transfer(&self, draw: Query<Draw>) -> std::result::Result<String, LedgerError> {
        draw.0.settle()
    }

    #[get("/statement")]
    #[public]
    #[response_header("x-ledger", "7")]
    async fn statement(&self, draw: Query<Draw>) -> std::result::Result<String, LedgerError> {
        draw.0.settle()
    }

    #[sse("/movements")]
    #[public]
    async fn movements(&self, draw: Query<Draw>) -> std::result::Result<SseStream, LedgerError> {
        let settled = draw.0.settle()?;
        Ok(SseStream::new(stream::iter([SseEvent::message(settled)])))
    }
}

#[input]
struct Draw {
    amount: u64,
}

impl Draw {
    fn settle(&self) -> std::result::Result<String, LedgerError> {
        if self.amount > 100 {
            return Err(LedgerError::Overdrawn);
        }
        Ok(format!("moved {}", self.amount))
    }
}

#[module(providers = [DomainController, Forwarding])]
struct DomainModule;

type Client = TestClient;

/// An interceptor that only forwards: listed by the app, it puts every route on
/// the layered pipeline, where its tail renders a handler's `Err`.
#[injectable]
#[derive(Default)]
struct Forwarding;

impl Layer for Forwarding {}

#[async_trait]
impl Interceptor for Forwarding {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[module(imports = [DecodeModule], providers = [GlobalInterceptors<(Forwarding,)>])]
struct WrappedDecodeModule;

#[module(imports = [FailureModule], providers = [GlobalInterceptors<(Forwarding,)>])]
struct WrappedFailureModule;

#[module(imports = [DomainModule], providers = [GlobalInterceptors<(Forwarding,)>])]
struct WrappedDomainModule;

/// The app every real deployment is: `W`, `M` with a layer listed.
async fn wrapped<W: Module + 'static>() -> Client {
    let app = App::builder()
        .module::<W>()
        .build()
        .await
        .expect("module boots");
    let mut transport = crate::built(&HttpConfig::default());
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    crate::client(
        transport
            .take_endpoint()
            .expect("configure populates the endpoint"),
    )
}

/// Every place an `Err` is rendered: the fused edge, the outer wrap compression
/// mounts, and the tail of a layered route, `W`.
async fn clients<M: Module + 'static, W: Module + 'static>() -> [Client; 3] {
    [
        crate::boot::<M>().await,
        crate::boot_on::<M>(HttpConfig {
            compression: true,
            ..HttpConfig::default()
        })
        .await,
        wrapped::<W>().await,
    ]
}

async fn detail(resp: nest_rs_testing::TestResponse) -> String {
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
    for client in clients::<DecodeModule, WrappedDecodeModule>().await {
        let resp = client
            .post("/decode/json")
            .content_type("application/json")
            .body(secret_json())
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "JSON body at `amount`: invalid type: a string, expected u64",
        );
        let resp = client
            .post("/decode/json")
            .content_type("application/json")
            .body(r#"{"amount":["x"]}"#)
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "JSON body at `amount`: invalid type: a sequence, expected u64",
        );
        let resp = client
            .post("/decode/json")
            .content_type("application/json")
            .body(format!(r#"{{"amount":1,"{SECRET}":1}}"#))
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "JSON body: unknown field, expected `amount`",
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
    for client in clients::<DecodeModule, WrappedDecodeModule>().await {
        let resp = client
            .get("/decode/query")
            .query("card", &SECRET)
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "query parameter `card`: unknown variant, expected `Visa`"
        );
        let resp = client
            .post("/decode/form")
            .content_type("application/x-www-form-urlencoded")
            .body(format!("card={SECRET}"))
            .send()
            .await;
        assert_eq!(
            detail(resp).await,
            "form field `card`: unknown variant, expected `Visa`"
        );
    }
}

#[tokio::test]
async fn a_handler_s_own_decode_failure_is_refused_without_its_value() {
    let body = format!(r#""{SECRET}""#);
    for client in clients::<DecodeModule, WrappedDecodeModule>().await {
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
/// the report, at `error`, while the client reads the opaque `500`.
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

async fn problem_json(resp: nest_rs_testing::TestResponse) -> serde_json::Value {
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
    taken.assert_header(nest_rs_http::header::RETRY_AFTER, "30");
    let body = problem_json(taken).await;
    assert_eq!(body["code"], "CONFLICT", "{body}");
    assert_eq!(body["detail"], "the handle is taken", "{body}");
}

/// A server `Problem` answers its status, its code and its wait, never the
/// chain behind it, which every path that renders it files once — a `503`'s at
/// `warn`, the server unable to serve it in time; a bare `Problem`, like a
/// bare status, withholds nothing and files nothing, and so does a client one.
#[tokio::test]
async fn a_server_problem_answers_its_status_and_files_the_chain_it_withholds() {
    for path in ["/failure/outage", "/failure/outage-carried"] {
        let logs = nest_rs_testing::LogCapture::install();
        let client = crate::boot::<FailureModule>().await;
        let resp = client.get(path).send().await;
        resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        resp.assert_header(nest_rs_http::header::RETRY_AFTER, "30");
        let body = problem_json(resp).await;
        assert_eq!(body["code"], "UNAVAILABLE", "{path}: {body}");
        assert!(!body.to_string().contains("10.0.0.1"), "{path}: {body}");

        let unavailable = logs.expect_one(nest_rs_http::target::HTTP, "request unavailable");
        assert_eq!(unavailable.level, "warn", "{path}");
        assert!(
            unavailable
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1") && e.contains("503 UNAVAILABLE")),
            "{path}: {unavailable:#?}",
        );
    }

    let logs = nest_rs_testing::LogCapture::install();
    let client = crate::boot::<FailureModule>().await;
    let bare = client.get("/failure/unavailable").send().await;
    bare.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    bare.assert_header(nest_rs_http::header::RETRY_AFTER, "30");
    assert_eq!(problem_json(bare).await["code"], "UNAVAILABLE");
    for path in ["/failure/carried", "/failure/taken"] {
        let resp = client.get(path).send().await;
        assert!(resp.0.status().is_client_error(), "{path}");
    }
    logs.expect_none(nest_rs_http::target::HTTP, "request failed");
    logs.expect_none(nest_rs_http::target::HTTP, "request unavailable");
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

/// Each chain the edge filed for a failed request: its level, its message
/// and the chain it said.
fn filed(logs: &nest_rs_testing::LogCapture) -> Vec<(String, String, Option<String>)> {
    logs.events()
        .into_iter()
        .filter(|event| event.target == nest_rs_http::target::HTTP)
        .filter(|event| event.message == "request failed" || event.message == "request unavailable")
        .map(|event| {
            let error = event.field("error");
            (event.level, event.message, error)
        })
        .collect()
}

/// A handler's own error answering a `5xx` through `ResponseError`'s default
/// files its whole chain once, at `error` — a `503`'s at `warn` — on every
/// path an `Err` is rendered by, a decode failure in it said without its
/// value; a typed `4xx` and the problem document returned as the error
/// withhold nothing and file nothing.
#[tokio::test]
async fn a_typed_server_error_files_its_chain_once_by_the_status_sent() {
    for client in clients::<FailureModule, WrappedFailureModule>().await {
        let logs = nest_rs_testing::LogCapture::install();
        client
            .get("/failure/typed")
            .send()
            .await
            .assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            filed(&logs),
            [(
                "error".to_owned(),
                "request failed".to_owned(),
                Some("the ledger at 10.0.0.1 holds secret 42".to_owned()),
            )],
        );

        let logs = nest_rs_testing::LogCapture::install();
        client
            .post("/failure/typed-decode")
            .body(format!(r#""{SECRET}""#))
            .send()
            .await
            .assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            filed(&logs),
            [(
                "error".to_owned(),
                "request failed".to_owned(),
                Some(
                    "the ledger entry does not decode: invalid type: a string, expected u64 at line 1 column 24"
                        .to_owned()
                ),
            )],
        );
        assert!(
            quoting(&logs, SECRET).is_empty(),
            "{:#?}",
            quoting(&logs, SECRET)
        );

        let logs = nest_rs_testing::LogCapture::install();
        let busy = client.get("/failure/typed-busy").send().await;
        busy.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(problem_json(busy).await["code"], "UNAVAILABLE");
        assert_eq!(
            filed(&logs),
            [(
                "warn".to_owned(),
                "request unavailable".to_owned(),
                Some("the ledger is busy: the ledger at 10.0.0.1 timed out".to_owned()),
            )],
        );

        let logs = nest_rs_testing::LogCapture::install();
        client
            .get("/failure/typed-refusal")
            .send()
            .await
            .assert_status(StatusCode::CONFLICT);
        client
            .get("/failure/document")
            .send()
            .await
            .assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(filed(&logs), [], "nothing withheld, nothing filed");
    }
}

/// A domain error writing `ToProblem` alone answers a route its problem — its
/// RFC 9457 `type` and `title`, its `code`, its `detail` and its field map —
/// whether the handler returns it or converts it into the edge's own error,
/// on every path an `Err` is rendered by; a client answer files nothing.
#[tokio::test]
async fn a_to_problem_error_answers_its_problem_from_a_route() {
    for client in clients::<DomainModule, WrappedDomainModule>().await {
        let logs = nest_rs_testing::LogCapture::install();
        for path in ["/domain/overdrawn", "/domain/converted"] {
            let resp = client.get(path).send().await;
            resp.assert_status(StatusCode::CONFLICT);
            resp.assert_content_type("application/problem+json");
            assert_eq!(
                problem_json(resp).await,
                serde_json::json!({
                    "type": "https://example.com/problems/overdrawn",
                    "title": "Account overdrawn",
                    "status": 409,
                    "detail": "account 7 is overdrawn",
                    "code": "CONFLICT",
                }),
                "{path}",
            );
        }
        let refused = client.get("/domain/refused").send().await;
        refused.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            problem_json(refused).await,
            serde_json::json!({
                "type": "about:blank",
                "title": "Unprocessable Content",
                "status": 422,
                "detail": "the transfer is refused",
                "code": "INVALID_ARGUMENT",
                "errors": { "amount": [{ "code": "range" }] },
            }),
        );
        assert_eq!(filed(&logs), [], "a client answer withholds nothing");
    }
}

/// A route a response decorator shapes, and an `#[sse]` open, read a
/// `ToProblem` error as every route does: a failure answers its problem,
/// none of the success's status or headers on it, and a success is shaped.
#[tokio::test]
async fn a_to_problem_error_answers_its_problem_from_a_shaped_route_and_an_sse_open() {
    for client in clients::<DomainModule, WrappedDomainModule>().await {
        for failed in [
            client.post("/domain/transfer?amount=500").send().await,
            client.get("/domain/statement?amount=500").send().await,
            client.get("/domain/movements?amount=500").send().await,
        ] {
            failed.assert_status(StatusCode::CONFLICT);
            failed.assert_content_type("application/problem+json");
            failed.assert_header_is_not_exist("x-ledger");
            assert_eq!(
                problem_json(failed).await,
                serde_json::json!({
                    "type": "https://example.com/problems/overdrawn",
                    "title": "Account overdrawn",
                    "status": 409,
                    "detail": "account 7 is overdrawn",
                    "code": "CONFLICT",
                }),
            );
        }

        let created = client.post("/domain/transfer?amount=5").send().await;
        created.assert_status(StatusCode::CREATED);
        created.assert_text("moved 5").await;
        let statement = client.get("/domain/statement?amount=5").send().await;
        statement.assert_status_is_ok();
        statement.assert_header("x-ledger", "7");
        statement.assert_text("moved 5").await;
        let movements = client.get("/domain/movements?amount=5").send().await;
        movements.assert_status_is_ok();
        let body = movements
            .0
            .into_body()
            .into_string()
            .await
            .unwrap_or_default();
        assert!(body.contains("data: moved 5"), "the stream opens: {body:?}");
    }
}

/// A `ToProblem` error answering a `5xx` keeps its chain from the client and
/// files it once — a `503`'s at `warn` with its wait, one saying no problem
/// at `error` behind the one opaque document.
#[tokio::test]
async fn a_to_problem_server_error_files_the_chain_it_withholds() {
    for client in clients::<DomainModule, WrappedDomainModule>().await {
        let logs = nest_rs_testing::LogCapture::install();
        let busy = client.get("/domain/busy").send().await;
        busy.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        busy.assert_header(nest_rs_http::header::RETRY_AFTER, "30");
        assert_eq!(problem_json(busy).await["code"], "UNAVAILABLE");
        assert_eq!(
            filed(&logs),
            [(
                "warn".to_owned(),
                "request unavailable".to_owned(),
                Some("the ledger is busy: the ledger at 10.0.0.1 timed out".to_owned()),
            )],
        );

        let logs = nest_rs_testing::LogCapture::install();
        let down = client.get("/domain/down").send().await;
        down.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(problem_json(down).await, internal_document());
        assert_eq!(
            filed(&logs),
            [(
                "error".to_owned(),
                "request failed".to_owned(),
                Some("the ledger at 10.0.0.1 holds secret 42".to_owned()),
            )],
        );
    }
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

/// The one document every opaque `500` is: the class's problem, its `code`
/// and no `detail`.
fn internal_document() -> serde_json::Value {
    serde_json::json!({
        "type": "about:blank",
        "title": "Internal Server Error",
        "status": 500,
        "code": "INTERNAL",
    })
}

/// Every server failure the framework answers on HTTP without describing it
/// is one document, whichever part answered — an error no one meant for the
/// client, a bare status, `Opaque`, a raw-text `500`, a panic, an error
/// rendered by `ResponseError`'s default, the class's own `Problem` — on every
/// path an `Err` is rendered by, and none of it quotes the cause.
#[tokio::test]
async fn every_opaque_500_is_one_document_whoever_answered_it() {
    for client in clients::<FailureModule, WrappedFailureModule>().await {
        for path in [
            "/failure/leaky",
            "/failure/wrapped",
            "/failure/ledger",
            "/failure/status",
            "/failure/opaque",
            "/failure/raw",
            "/failure/panic",
            "/failure/typed",
            "/failure/problem",
        ] {
            let resp = client.get(path).send().await;
            resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
            resp.assert_content_type("application/problem+json");
            let body = problem_json(resp).await;
            assert_eq!(body, internal_document(), "{path}");
            assert!(!body.to_string().contains("secret"), "{path}: {body}");
        }
    }
}

/// A `404` answered from its status alone is its class's document, the code
/// `#[crud]` and the in-band edges name it by, whoever answered it — a bare
/// status, a raw-text body, the class's own `Problem` a chain carries, the
/// router for an address nothing serves — on every path an `Err` is rendered by.
#[tokio::test]
async fn every_bare_404_is_its_class_s_document_whoever_answered_it() {
    for client in clients::<FailureModule, WrappedFailureModule>().await {
        for path in [
            "/failure/missing",
            "/failure/raw-missing",
            "/failure/carried",
            "/failure/nowhere",
        ] {
            let resp = client.get(path).send().await;
            resp.assert_status(StatusCode::NOT_FOUND);
            resp.assert_content_type("application/problem+json");
            assert_eq!(
                problem_json(resp).await,
                serde_json::json!({
                    "type": "about:blank",
                    "title": "Not Found",
                    "status": 404,
                    "code": "NOT_FOUND",
                }),
                "{path}",
            );
        }
    }
}

/// A `503` the framework answers without describing it is its class's
/// document: `UNAVAILABLE`, and no `detail`.
#[tokio::test]
async fn an_opaque_503_is_its_class_s_document() {
    for client in clients::<FailureModule, WrappedFailureModule>().await {
        let resp = client.get("/failure/busy").send().await;
        resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            problem_json(resp).await,
            serde_json::json!({
                "type": "about:blank",
                "title": "Service Unavailable",
                "status": 503,
                "code": "UNAVAILABLE",
            }),
        );
    }
}
