//! Covers `src/error.rs`: `.opaque()` and `pipe_error` never say a refused value.

use nest_rs_core::anyhow::{self, Context};
use nest_rs_core::module;
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpOperationGuard, Opaque, Parameters, Piped, input, mcp, tools,
};
use nest_rs_pipes::{ParseArray, Pipe, PipeError};
use nest_rs_testing::mcp::call_tool_with;
use nest_rs_testing::{LogCapture, TestApp};
use serde::de::Error as _;
use serde::de::value::Error as ValueError;

const SECRET: &str = "sk_live_51HsecretTOKEN";

/// What the operator's line says of `failed`, after checking the model is told
/// nothing of it.
fn said(failed: anyhow::Result<u64>) -> String {
    let logs = LogCapture::install();
    let refused = failed
        .opaque()
        .expect_err("the failure survives as a failure");
    assert!(
        !format!("{refused:?}").contains(SECRET),
        "the model is told nothing of it: {refused:?}"
    );
    logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed")
        .field("error")
        .expect("the line carries the cause")
}

/// serde's own quoting sentence, behind `?` and behind a `.context(…)`.
#[test]
fn an_anyhow_chained_decode_failure_is_said_without_its_value() {
    let body = format!("\"{SECRET}\"");
    let bare: anyhow::Result<u64> = serde_json::from_str(&body).map_err(anyhow::Error::from);
    let text = said(bare);
    assert!(!text.contains(SECRET), "{text}");
    assert!(
        text.contains("expected u64"),
        "what failed is still said: {text}"
    );

    let text = said(serde_json::from_str(&body).context("the upstream reply"));
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.starts_with("the upstream reply: "), "{text}");
}

/// anyhow's own box hides the error it holds from `source()`: this fails when
/// `.opaque()` boxes with `.into()` rather than `boxed_error`.
#[test]
fn a_decode_failure_in_a_type_s_own_words_is_said_without_its_value() {
    let refused = ValueError::custom(format!("token {SECRET} is not ours"));
    let text = said(Err(anyhow::Error::new(refused)));
    assert!(!text.contains(SECRET), "{text}");
}

/// A tool's arguments arrive as an object, so a list rides in a field and the
/// operation's pipe hands that field to `ParseArray`.
#[input]
struct IdListArgs {
    ids: String,
}

struct ParseIdList;

impl Pipe for ParseIdList {
    type In = IdListArgs;
    type Out = Vec<u64>;
    fn transform(input: IdListArgs) -> Result<Vec<u64>, PipeError> {
        ParseArray::<u64>::transform(input.ids)
    }
}

#[mcp(path = "/mcp/id-lists")]
#[derive(Clone, Default)]
struct IdListTool;

#[tools]
impl IdListTool {
    /// Count the ids in a comma-separated list.
    #[tool]
    #[public]
    async fn count_ids(
        &self,
        Parameters(ids): Parameters<Piped<ParseIdList, IdListArgs>>,
    ) -> Result<String, McpError> {
        Ok(ids.into_inner().len().to_string())
    }
}

#[module(providers = [IdListTool, AllowAllMcpGuard as dyn McpOperationGuard])]
struct IdListModule;

/// `pipe_error` hands a pipe's refusal to the model as it is.
#[tokio::test]
async fn a_refused_list_item_is_never_quoted_to_the_model() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<IdListModule>()
        .await
        .expect("a host taking a piped list boots");

    let body = call_tool_with(
        app.http(),
        "/mcp/id-lists",
        "count_ids",
        None,
        serde_json::json!({ "ids": format!("1,{SECRET},3") }),
    )
    .await;

    assert!(
        !body.contains("sk_live"),
        "the reply quotes the item: {body}"
    );
    assert!(
        body.contains("u64"),
        "the refusal names what an item must be: {body}"
    );
    // rmcp's own trace and debug lines print every request whole, before any
    // pipe runs: they are the dependency's, not this edge's.
    let quoting: Vec<String> = logs
        .events()
        .into_iter()
        .filter(|event| !event.target.starts_with("rmcp"))
        .filter(|event| event.fields.values().any(|value| value.contains("sk_live")))
        .map(|event| format!("{} {} {:?}", event.target, event.message, event.fields))
        .collect();
    assert!(quoting.is_empty(), "lines quoting the item: {quoting:#?}");
}
