//! `Parameters<T>` refuses arguments that do not decode without the value the
//! client sent — on a tool, where the refusal reaches the model as a tool error
//! it can correct, and on a prompt.
//!
//! rmcp's own extractor answered `failed to deserialize parameters: invalid
//! type: string "…", expected u64`, which quotes the argument into the model's
//! transcript.

use nest_rs_core::module;
use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Role};
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::{AllowAllMcpGuard, McpError, McpOperationGuard, Parameters, input, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_method, call_tool_with, open_session, result};

const PATH: &str = "/mcp/charges";

const SECRET: &str = "sk_live_51HsecretTOKEN";

#[input]
struct ChargeArgs {
    amount: u64,
}

#[mcp(path = "/mcp/charges")]
#[derive(Clone, Default)]
struct ChargeTool;

#[tools]
impl ChargeTool {
    /// Charge an amount.
    #[tool]
    #[public]
    async fn charge(&self, Parameters(args): Parameters<ChargeArgs>) -> Result<String, McpError> {
        Ok(args.amount.to_string())
    }

    /// Draft a receipt for an amount.
    #[prompt]
    #[public]
    async fn receipt(
        &self,
        Parameters(args): Parameters<ChargeArgs>,
    ) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            args.amount.to_string(),
        )]))
    }
}

#[module(providers = [ChargeTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct ChargeModule;

async fn boot() -> TestApp {
    TestApp::for_module::<ChargeModule>()
        .await
        .expect("a host taking typed arguments boots")
}

/// A tool's arguments that do not decode are refused as a tool result flagged
/// `isError` — the form the MCP specification gives an input error, so the model
/// can correct its call — naming where and what kind, never the value. A key
/// the client spelled is the client's too.
#[tokio::test]
async fn a_tool_s_arguments_that_do_not_decode_are_refused_without_their_value() {
    let app = boot().await;
    for (arguments, said) in [
        (
            json!({ "amount": SECRET }),
            "failed to deserialize parameters: invalid type: a string, expected u64",
        ),
        (
            json!({ "amount": 1, SECRET: 1 }),
            "failed to deserialize parameters: unknown field, expected `amount`",
        ),
    ] {
        let body = call_tool_with(app.http(), PATH, "charge", None, arguments).await;
        let answer = result(&body);
        assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
        assert_eq!(
            answer["result"]["content"][0]["text"],
            json!(said),
            "{answer}"
        );
        assert!(!body.contains(SECRET), "{body}");
    }

    let body = call_tool_with(app.http(), PATH, "charge", None, json!({ "amount": 7 })).await;
    assert_eq!(result(&body)["result"]["content"][0]["text"], json!("7"));
}

/// A prompt's arguments that do not decode are refused as `invalid_params`, the
/// same sentence and no value.
#[tokio::test]
async fn a_prompt_s_arguments_that_do_not_decode_are_refused_without_their_value() {
    let app = boot().await;
    let session = open_session(app.http(), PATH, None).await;
    let body = call_method(
        app.http(),
        PATH,
        &session,
        None,
        "prompts/get",
        json!({ "name": "receipt", "arguments": { "amount": SECRET } }),
    )
    .await;
    let answer = result(&body);
    assert_eq!(answer["error"]["code"], json!(-32602), "{answer}");
    assert_eq!(
        answer["error"]["message"],
        json!("failed to deserialize parameters: invalid type: a string, expected u64"),
    );
    assert!(!body.contains(SECRET), "{body}");
}
