//! One chain, one answer, on every edge: `Problem` is the one vocabulary an
//! edge reads in a handler's error. An upstream whose answer does not decode is
//! the server's failure — `INTERNAL`, its chain filed once — never the
//! client's; a handler decoding the client's own text by hand says so with
//! `Problem::from(DecodeError)`, answered `INVALID_ARGUMENT` where it failed
//! and without the value; and a pipe's refusal a handler returns carries its
//! problem, so it answers `INVALID_ARGUMENT` wherever it was wrapped.

use std::sync::LazyLock;

use nest_rs_core::{DecodeError, OPAQUE_CLIENT_MESSAGE, Problem, input, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{Json, controller, routes};
use nest_rs_mcp::{Parameters, mcp, tools};
use nest_rs_pipes::{ParseUuid, Pipe};
use nest_rs_testing::mcp::{call_tool_statelessly, result};
use nest_rs_testing::ws::{WsApp, WsSocket};
use nest_rs_testing::{CapturedEvent, HttpsPeer, LogCapture, PeerAnswer, TestApp, TestAuthority};
use nest_rs_ws::{WsModule, gateway, messages};
use serde::Deserialize;
use serde_json::{Value, json};

/// What only the upstream's answer and the client's text hold: neither a reply
/// nor a line may repeat it.
const SECRET: &str = "sk_live_51H";

/// The authority the upstream's certificate chains to, and the handlers'
/// client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

#[derive(Debug, Deserialize)]
struct Parcel {
    weight: u32,
}

#[derive(Debug, Deserialize)]
struct Order {
    quantity: u32,
}

/// What a handler calling a partner writes: the partner's answer decoded with
/// `?`, whatever it holds.
async fn weighed(url: &str) -> anyhow::Result<String> {
    let authority = reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes())?;
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .build()?;
    let parcel: Parcel = client.get(url).send().await?.json().await?;
    Ok(parcel.weight.to_string())
}

/// What a handler decoding its client's text by hand writes: the failure said
/// as the client's, located in the type it was read as.
fn ordered(text: &str) -> Result<String, Problem> {
    let order: Order = serde_json::from_str(text).map_err(|failure| {
        let again = &mut serde_json::Deserializer::from_str(text);
        Problem::from(DecodeError::from(failure).located::<Order, _>(again))
    })?;
    Ok(order.quantity.to_string())
}

/// A pipe run by hand in a handler's body: its refusal returned with `?`.
fn identified(text: String) -> anyhow::Result<String> {
    Ok(ParseUuid::transform(text)?.to_string())
}

#[controller(path = "/chain")]
#[derive(Default)]
struct ChainController;

#[routes]
impl ChainController {
    #[post("/upstream")]
    #[public]
    async fn upstream(&self, text: Json<String>) -> anyhow::Result<String> {
        weighed(&text.0).await
    }

    #[post("/decoded")]
    #[public]
    async fn decoded(&self, text: Json<String>) -> anyhow::Result<String> {
        Ok(ordered(&text.0)?)
    }

    #[post("/piped")]
    #[public]
    async fn piped(&self, text: Json<String>) -> anyhow::Result<String> {
        identified(text.0)
    }
}

#[resolver]
struct ChainResolver;

#[operations]
impl ChainResolver {
    #[query]
    #[public]
    async fn upstream(&self, text: String) -> anyhow::Result<String> {
        weighed(&text).await
    }

    #[query]
    #[public]
    async fn decoded(&self, text: String) -> anyhow::Result<String> {
        Ok(ordered(&text)?)
    }

    #[query]
    #[public]
    async fn piped(&self, text: String) -> anyhow::Result<String> {
        identified(text)
    }
}

#[input]
struct TextArgs {
    text: String,
}

#[mcp]
#[derive(Clone, Default)]
struct ChainTools;

#[tools]
impl ChainTools {
    /// Calls the partner.
    #[tool]
    #[public]
    async fn upstream(&self, Parameters(args): Parameters<TextArgs>) -> anyhow::Result<String> {
        weighed(&args.text).await
    }

    /// Decodes the client's text by hand.
    #[tool]
    #[public]
    async fn decoded(&self, Parameters(args): Parameters<TextArgs>) -> anyhow::Result<String> {
        Ok(ordered(&args.text)?)
    }

    /// Runs a pipe by hand.
    #[tool]
    #[public]
    async fn piped(&self, Parameters(args): Parameters<TextArgs>) -> anyhow::Result<String> {
        identified(args.text)
    }
}

#[gateway(path = "/ws/chain")]
struct ChainGateway;

#[messages]
impl ChainGateway {
    #[subscribe_message("upstream")]
    #[public]
    async fn upstream(&self, text: String) -> anyhow::Result<String> {
        weighed(&text).await
    }

    #[subscribe_message("decoded")]
    #[public]
    async fn decoded(&self, text: String) -> anyhow::Result<String> {
        Ok(ordered(&text)?)
    }

    #[subscribe_message("piped")]
    #[public]
    async fn piped(&self, text: String) -> anyhow::Result<String> {
        identified(text)
    }
}

fn open_mcp() -> nest_rs_mcp::McpSetup {
    nest_rs_mcp::McpModule::for_root(nest_rs_mcp::McpAccess::Public)
}

#[module(
    imports = [GraphqlModule, WsModule, open_mcp()],
    providers = [ChainController, ChainResolver, ChainTools, ChainGateway],
)]
struct EveryEdgeModule;

/// What one edge answered `operation` called with `text`: the class it names,
/// the sentence its client reads, and everything it sent, as text.
struct Answer {
    edge: &'static str,
    code: Value,
    sentence: Value,
    whole: String,
}

/// The `warn` and `error` lines the framework filed since `seen` of them were
/// — rmcp's own `response error`, which the library files beside any JSON-RPC
/// error it answers, is the library's.
fn filed_since(logs: &LogCapture, seen: usize) -> Vec<CapturedEvent> {
    logs.events()
        .into_iter()
        .filter(|event| event.level == "warn" || event.level == "error")
        .filter(|event| !event.target.starts_with("rmcp"))
        .skip(seen)
        .collect()
}

/// `operation` called with `text` on each of the four edges in turn, each
/// answer beside the lines its call filed.
async fn called(
    logs: &LogCapture,
    app: &TestApp,
    socket: &mut WsSocket,
    operation: &str,
    text: &str,
) -> Vec<(Answer, Vec<CapturedEvent>)> {
    let mut answers = Vec::new();

    let seen = filed_since(logs, 0).len();
    let whole = app
        .http()
        .post(format!("/chain/{operation}"))
        .body_json(&json!(text))
        .send()
        .await
        .0
        .into_body()
        .into_string()
        .await
        .expect("a body");
    let http: Value = serde_json::from_str(&whole).expect("a problem document");
    answers.push((
        Answer {
            edge: "HTTP",
            code: http["code"].clone(),
            sentence: http["detail"].clone(),
            whole,
        },
        filed_since(logs, seen),
    ));

    let seen = filed_since(logs, 0).len();
    let whole = app
        .http()
        .post("/graphql")
        .body_json(&json!({
            "query": format!("query($text: String!) {{ {operation}(text: $text) }}"),
            "variables": { "text": text },
        }))
        .send()
        .await
        .0
        .into_body()
        .into_string()
        .await
        .expect("a body");
    let graphql: Value = serde_json::from_str(&whole).expect("a GraphQL response");
    let error = &graphql["errors"][0];
    answers.push((
        Answer {
            edge: "GraphQL",
            code: error["extensions"]["code"].clone(),
            sentence: error["message"].clone(),
            whole,
        },
        filed_since(logs, seen),
    ));

    let seen = filed_since(logs, 0).len();
    let whole = call_tool_statelessly(
        app.http(),
        "/mcp",
        operation,
        None,
        &[],
        json!({ "text": text }),
    )
    .await;
    let error = &result(&whole)["error"];
    answers.push((
        Answer {
            edge: "MCP",
            code: error["data"]["code"].clone(),
            sentence: error["message"].clone(),
            whole,
        },
        filed_since(logs, seen),
    ));

    let seen = filed_since(logs, 0).len();
    socket.send(operation, json!(text)).await;
    let frame = socket.next_envelope().await;
    answers.push((
        Answer {
            edge: "WS",
            code: frame["data"]["code"].clone(),
            sentence: frame["data"]["error"].clone(),
            whole: frame.to_string(),
        },
        filed_since(logs, seen),
    ));

    answers
}

/// The app on both harnesses, and a socket on its gateway.
async fn every_edge() -> (TestApp, WsApp, WsSocket) {
    let app = TestApp::for_module::<EveryEdgeModule>()
        .await
        .expect("every edge boots");
    let ws = TestApp::builder()
        .module::<EveryEdgeModule>()
        .build_ws()
        .await
        .expect("every edge serves");
    let socket = ws.socket("/ws/chain").connect().await;
    (app, ws, socket)
}

#[tokio::test]
async fn an_upstream_answer_that_does_not_decode_is_the_servers_failure_on_every_edge() {
    let logs = LogCapture::install();
    let peer = HttpsPeer::answering(
        &AUTHORITY.server(&["127.0.0.1"]),
        [PeerAnswer::body(200, format!(r#"{{"weight":"{SECRET}"}}"#))
            .header("content-type", "application/json")],
    )
    .await;
    let (app, ws, mut socket) = every_edge().await;

    let url = peer.url("/parcels/42");
    for (answer, lines) in called(&logs, &app, &mut socket, "upstream", &url).await {
        let Answer {
            edge,
            code,
            sentence,
            whole,
        } = answer;
        assert_eq!(code, "INTERNAL", "{edge}: {whole}");
        if edge == "HTTP" {
            assert_eq!(sentence, Value::Null, "{edge}: a `5xx` says no detail");
        } else {
            assert_eq!(sentence, OPAQUE_CLIENT_MESSAGE, "{edge}: {whole}");
        }
        assert!(!whole.contains("invalid type"), "{edge}: {whole}");
        assert!(!whole.contains(SECRET), "{edge}: {whole}");

        let said: Vec<_> = lines
            .iter()
            .map(|line| (line.level.as_str(), line.message.as_str()))
            .collect();
        assert_eq!(lines.len(), 1, "{edge}: one failure, one line — {said:?}");
        assert_eq!(lines[0].level, "error", "{edge}: {said:?}");
        let chain = lines[0].field("error").unwrap_or_default();
        assert!(
            chain.contains("invalid type: a string, expected u32"),
            "{edge}: the line says what the upstream answered, by kind — {chain}"
        );
        assert!(!chain.contains(SECRET), "{edge}: {chain}");
    }
    assert!(!logs.quotes(SECRET));

    drop(socket);
    ws.close().await;
}

#[tokio::test]
async fn a_clients_text_a_handler_decodes_by_hand_is_refused_where_it_failed_on_every_edge() {
    let logs = LogCapture::install();
    let (app, ws, mut socket) = every_edge().await;

    let text = format!(r#"{{"quantity":"{SECRET}"}}"#);
    for (answer, lines) in called(&logs, &app, &mut socket, "decoded", &text).await {
        let Answer {
            edge,
            code,
            sentence,
            whole,
        } = answer;
        assert_eq!(code, "INVALID_ARGUMENT", "{edge}: {whole}");
        assert_eq!(
            sentence, "at `quantity`: invalid type: a string, expected u32",
            "{edge}: {whole}"
        );
        assert!(!whole.contains(SECRET), "{edge}: {whole}");
        assert!(lines.is_empty(), "{edge}: a refusal files no line");
    }
    // rmcp's own `debug` and `trace` events quote the request they read, which
    // `nest-rs-mcp`'s clamp keeps from every subscriber the framework ships.
    let quoting: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| !event.target.starts_with("rmcp") && event.quotes(SECRET))
        .map(|event| format!("{} {} {}", event.level, event.target, event.message))
        .collect();
    assert!(quoting.is_empty(), "{quoting:#?}");

    drop(socket);
    ws.close().await;
}

#[tokio::test]
async fn a_pipes_refusal_a_handler_returns_answers_its_problem_on_every_edge() {
    let logs = LogCapture::install();
    let (app, ws, mut socket) = every_edge().await;

    for (answer, lines) in called(&logs, &app, &mut socket, "piped", "not-a-uuid").await {
        let Answer {
            edge,
            code,
            sentence,
            whole,
        } = answer;
        assert_eq!(code, "INVALID_ARGUMENT", "{edge}: {whole}");
        assert_eq!(sentence, "must be a valid UUID", "{edge}: {whole}");
        assert!(lines.is_empty(), "{edge}: a refusal files no line");
    }

    drop(socket);
    ws.close().await;
}

/// JSON-RPC names a refused argument `-32602`, whoever refused it: the edge
/// decoding the call's arguments, a pipe, or a handler saying so by hand.
#[tokio::test]
async fn an_invalid_argument_a_tool_says_keeps_json_rpcs_invalid_params() {
    let app = TestApp::for_module::<EveryEdgeModule>()
        .await
        .expect("every edge boots");

    for (operation, text) in [("decoded", "{}"), ("piped", "not-a-uuid")] {
        let body = call_tool_statelessly(
            app.http(),
            "/mcp",
            operation,
            None,
            &[],
            json!({ "text": text }),
        )
        .await;
        assert_eq!(
            result(&body)["error"]["code"],
            -32602,
            "{operation}: {body}"
        );
    }
}
