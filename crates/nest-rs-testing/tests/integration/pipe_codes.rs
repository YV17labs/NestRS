//! One pipe refusal, one class, on every edge: a per-argument pipe's refusal
//! names `INVALID_ARGUMENT` beside its field map — the problem document's
//! `code` and `errors` on HTTP, `extensions.code` and `extensions.errors` on
//! GraphQL, `data.code` and `data.errors` in a WebSocket frame and in an MCP
//! error — and the class alone where the refusal has no field map.

use std::marker::PhantomData;

use nest_rs_core::{input, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{Json, controller, routes};
use nest_rs_mcp::{McpError, Parameters, mcp, tools};
use nest_rs_pipes::{Pipe, PipeError, Piped};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_tool_statelessly, result};
use nest_rs_ws::{WsModule, gateway, messages};
use serde_json::{Value, json};

/// The field map a [`Detailed`] refusal carries.
fn field_map() -> Value {
    json!({ "text": [{ "code": "length", "message": "text must not be empty" }] })
}

/// Refuses whatever it is handed, naming the field it would fix.
struct Detailed<T>(PhantomData<T>);

impl<T> Pipe for Detailed<T> {
    type In = T;
    type Out = T;

    fn transform(_: T) -> Result<T, PipeError> {
        Err(PipeError::with_details("validation failed", field_map()))
    }
}

/// Refuses whatever it is handed, in a sentence alone.
struct Bare<T>(PhantomData<T>);

impl<T> Pipe for Bare<T> {
    type In = T;
    type Out = T;

    fn transform(_: T) -> Result<T, PipeError> {
        Err(PipeError::new("must be a valid UUID"))
    }
}

#[controller(path = "/piped")]
#[derive(Default)]
struct PipedController;

#[routes]
impl PipedController {
    #[post("/detailed")]
    #[public]
    async fn detailed(&self, text: Piped<Detailed<String>, Json<String>>) -> String {
        text.into_inner()
    }

    #[post("/bare")]
    #[public]
    async fn bare(&self, text: Piped<Bare<String>, Json<String>>) -> String {
        text.into_inner()
    }
}

#[resolver]
struct PipedResolver;

#[operations]
impl PipedResolver {
    #[query]
    #[public]
    async fn detailed(&self, text: Piped<Detailed<String>>) -> GqlResult<String> {
        Ok(text.into_inner())
    }

    #[query]
    #[public]
    async fn bare(&self, text: Piped<Bare<String>>) -> GqlResult<String> {
        Ok(text.into_inner())
    }
}

#[input]
struct TextArgs {
    text: String,
}

#[mcp]
#[derive(Clone, Default)]
struct PipedTools;

#[tools]
impl PipedTools {
    /// Refused with a field map.
    #[tool]
    #[public]
    async fn detailed(
        &self,
        Parameters(args): Parameters<Piped<Detailed<TextArgs>>>,
    ) -> Result<String, McpError> {
        Ok(args.into_inner().text)
    }

    /// Refused in a sentence.
    #[tool]
    #[public]
    async fn bare(
        &self,
        Parameters(args): Parameters<Piped<Bare<TextArgs>>>,
    ) -> Result<String, McpError> {
        Ok(args.into_inner().text)
    }
}

#[gateway(path = "/ws/piped")]
struct PipedGateway;

#[messages]
impl PipedGateway {
    #[subscribe_message("detailed")]
    #[public]
    async fn detailed(&self, text: Piped<Detailed<String>>) -> String {
        text.into_inner()
    }

    #[subscribe_message("bare")]
    #[public]
    async fn bare(&self, text: Piped<Bare<String>>) -> String {
        text.into_inner()
    }
}

/// The endpoint opened as an app opens it in code, so the tools are reached
/// with no guard in front of them.
fn open_mcp() -> nest_rs_mcp::McpSetup {
    nest_rs_mcp::McpModule::for_root(nest_rs_mcp::McpAccess::Public)
}

#[module(
    imports = [GraphqlModule, WsModule, open_mcp()],
    providers = [PipedController, PipedResolver, PipedTools, PipedGateway],
)]
struct EveryEdgeModule;

#[tokio::test]
async fn a_pipe_refusal_names_its_class_beside_its_field_map_on_every_edge() {
    let app = TestApp::for_module::<EveryEdgeModule>()
        .await
        .expect("every edge boots");
    let ws = TestApp::builder()
        .module::<EveryEdgeModule>()
        .build_ws()
        .await
        .expect("every edge serves");
    let mut socket = ws.socket("/ws/piped").connect().await;

    for (operation, errors) in [("detailed", Some(field_map())), ("bare", None)] {
        let http: Value = app
            .http()
            .post(format!("/piped/{operation}"))
            .body_json(&json!("x"))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize();

        let graphql: Value = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": format!(r#"{{ {operation}(text: "x") }}"#) }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize();
        let graphql = &graphql["errors"][0]["extensions"];

        let mcp = result(
            &call_tool_statelessly(
                app.http(),
                "/mcp",
                operation,
                None,
                &[],
                json!({ "text": "x" }),
            )
            .await,
        );
        let mcp = &mcp["error"]["data"];

        socket.send(operation, json!("x")).await;
        let frame = socket.next_envelope().await;
        let ws_data = &frame["data"];

        for (edge, said) in [
            ("HTTP", &http),
            ("GraphQL", graphql),
            ("MCP", mcp),
            ("WS", ws_data),
        ] {
            assert_eq!(
                said["code"], "INVALID_ARGUMENT",
                "{operation} on {edge}: {said}"
            );
            assert_eq!(
                said.get("errors"),
                errors.as_ref(),
                "{operation} on {edge}: {said}",
            );
        }
    }
    drop(socket);
    ws.close().await;
}
