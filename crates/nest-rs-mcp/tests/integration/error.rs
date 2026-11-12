//! Covers `src/error.rs`: `.opaque()` and `pipe_error` never say a refused value.

use nest_rs_core::module;
use nest_rs_core::{
    anyhow::{self, Context},
    input,
};
use nest_rs_mcp::{McpError, Opaque, Parameters, mcp, tools};
use nest_rs_pipes::{ParseArray, Pipe, PipeError, Piped};
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
        Parameters(ids): Parameters<Piped<ParseIdList>>,
    ) -> Result<String, McpError> {
        Ok(ids.into_inner().len().to_string())
    }
}

#[module(imports = [crate::open_mcp()], providers = [IdListTool])]
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

/// What a tool keeps from the model, in the words an operator reads.
const LEDGER_REFUSED: &str = "the ledger at the eu-west warehouse refused";

/// A dependency that did not answer in time, which answers `503`.
#[derive(Debug, thiserror::Error)]
#[error("the warehouse at 10.0.0.1 did not answer")]
struct WarehouseDown;

impl nest_rs_core::ToProblem for WarehouseDown {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        Some(nest_rs_core::Problem::new(
            503,
            nest_rs_core::code::UNAVAILABLE,
        ))
    }
}

const WITHHELD: &str = "/mcp/withheld";

#[mcp(path = "/mcp/withheld")]
#[derive(Clone, Default)]
struct WithheldTools;

#[tools]
impl WithheldTools {
    /// Settle the ledger, through `.opaque()`.
    #[tool]
    #[public]
    async fn opaqued(&self) -> Result<String, McpError> {
        let settled: Result<u64, std::io::Error> = Err(std::io::Error::other(LEDGER_REFUSED));
        Ok(format!("settled {}", settled.opaque()?))
    }

    /// Settle the ledger, its error answered by the edge.
    #[tool]
    #[public]
    async fn failed(&self) -> Result<String, std::io::Error> {
        Err(std::io::Error::other(LEDGER_REFUSED))
    }

    /// Reach the warehouse, which did not answer.
    #[tool]
    #[public]
    async fn unavailable(&self) -> Result<String, WarehouseDown> {
        Err(WarehouseDown)
    }
}

/// Answers whatever it catches as the caller's conflict.
#[nest_rs_core::injectable]
#[derive(Default, nest_rs_core::Layer)]
struct AsConflict;

#[nest_rs_core::async_trait]
impl nest_rs_core::ExceptionFilter for AsConflict {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(
        &self,
        _: &Self::Exception,
        _: &mut nest_rs_core::UnitContext<'_>,
    ) -> nest_rs_core::Caught {
        nest_rs_core::Caught::Problem(nest_rs_core::Problem::new(
            409,
            nest_rs_core::code::CONFLICT,
        ))
    }
}

const CAUGHT: &str = "/mcp/withheld-and-caught";

#[mcp(path = "/mcp/withheld-and-caught")]
#[derive(Clone, Default)]
struct CaughtTools;

#[tools]
impl CaughtTools {
    /// Settle the ledger, through `.opaque()`, a filter answering it.
    #[tool]
    #[public]
    #[use_exception_filters(AsConflict)]
    async fn opaqued(&self) -> Result<String, McpError> {
        let settled: Result<u64, std::io::Error> = Err(std::io::Error::other(LEDGER_REFUSED));
        Ok(format!("settled {}", settled.opaque()?))
    }
}

/// Answers whatever it catches as the server unable to serve it in time.
#[nest_rs_core::injectable]
#[derive(Default, nest_rs_core::Layer)]
struct AsUnavailable;

#[nest_rs_core::async_trait]
impl nest_rs_core::ExceptionFilter for AsUnavailable {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(
        &self,
        _: &Self::Exception,
        _: &mut nest_rs_core::UnitContext<'_>,
    ) -> nest_rs_core::Caught {
        nest_rs_core::Caught::Problem(nest_rs_core::Problem::new(
            503,
            nest_rs_core::code::UNAVAILABLE,
        ))
    }
}

/// What the store refused once the cache had: the failure a filter answers.
const STORE_REFUSED: &str = "the store at the eu-west warehouse refused";

const KEPT: &str = "/mcp/withheld-and-kept";

#[mcp(path = "/mcp/withheld-and-kept")]
#[derive(Clone, Default)]
struct KeptTools;

#[tools]
impl KeptTools {
    /// Settle the ledger, its `.opaque()` failure reported in band, as the
    /// specification reports a tool's.
    #[tool]
    #[public]
    async fn in_band(&self) -> Result<nest_rs_mcp::model::CallToolResult, McpError> {
        use nest_rs_mcp::model::{CallToolResult, ContentBlock};

        let settled: Result<u64, std::io::Error> = Err(std::io::Error::other(LEDGER_REFUSED));
        Ok(match settled.opaque() {
            Ok(settled) => CallToolResult::success(vec![ContentBlock::text(settled.to_string())]),
            Err(refused) => CallToolResult::error(vec![ContentBlock::text(refused.message)]),
        })
    }

    /// Settle the ledger through `.opaque()`, a filter answering it `503`.
    #[tool]
    #[public]
    #[use_exception_filters(AsUnavailable)]
    async fn opaqued(&self) -> Result<String, McpError> {
        let settled: Result<u64, std::io::Error> = Err(std::io::Error::other(LEDGER_REFUSED));
        Ok(format!("settled {}", settled.opaque()?))
    }

    /// Read the ledger's cache through `.opaque()`, a miss handled, then fail
    /// on the store, a filter answering that failure `503`.
    #[tool]
    #[public]
    #[use_exception_filters(AsUnavailable)]
    async fn fallback(&self) -> Result<String, std::io::Error> {
        let cached: Result<u64, std::io::Error> = Err(std::io::Error::other(LEDGER_REFUSED));
        if let Ok(cached) = cached.opaque() {
            return Ok(cached.to_string());
        }
        Err(std::io::Error::other(STORE_REFUSED))
    }
}

#[module(
    imports = [crate::open_mcp()],
    providers = [WithheldTools, CaughtTools, KeptTools, AsConflict, AsUnavailable],
)]
struct WithheldModule;

/// Every chain an operation's answer withholds is filed naming the operation
/// as its `mcp.operation` line does — its mount's `path` and its tool — at
/// `error`, or at `warn` behind a `503`, whoever rendered it: the edge, or
/// `.opaque()` at its call site, which hands its chain to the edge.
#[tokio::test]
async fn a_withheld_chain_is_filed_naming_its_operation() {
    let app = TestApp::for_module::<WithheldModule>()
        .await
        .expect("the hosts boot");
    for (tool, level, message, chain) in [
        ("opaqued", "error", "mcp operation failed", LEDGER_REFUSED),
        ("failed", "error", "mcp operation failed", LEDGER_REFUSED),
        (
            "unavailable",
            "warn",
            "mcp operation unavailable",
            "the warehouse at 10.0.0.1 did not answer",
        ),
    ] {
        let logs = LogCapture::install();
        let body = nest_rs_testing::mcp::call_tool(app.http(), WITHHELD, tool, None).await;
        assert!(
            !body.contains("eu-west") && !body.contains("10.0.0.1"),
            "{body}"
        );

        let line = logs.expect_one(nest_rs_mcp::TARGET, message);
        assert_eq!(line.level, level, "{tool}");
        assert_eq!(line.field("path").as_deref(), Some(WITHHELD), "{tool}");
        assert_eq!(line.field("tool").as_deref(), Some(tool), "{tool}");
        assert_eq!(line.field("error").as_deref(), Some(chain), "{tool}");
    }
}

/// A failure an exception filter answers as the caller's files nothing of the
/// chain `.opaque()` kept, as for any failure answered `4xx`.
#[tokio::test]
async fn a_chain_opaque_withholds_is_filed_only_when_its_answer_goes_out() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<WithheldModule>()
        .await
        .expect("the hosts boot");

    let body = nest_rs_testing::mcp::call_tool(app.http(), CAUGHT, "opaqued", None).await;
    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(answer["error"]["data"]["code"], "CONFLICT", "{answer}");

    logs.expect_none(nest_rs_mcp::TARGET, "mcp operation failed");
}

/// A chain `.opaque()` kept is filed once the operation ends whatever it
/// answered: a tool error reported in band is a failed call, its chain on the
/// operator's line beside it.
#[tokio::test]
async fn a_chain_opaque_kept_is_filed_when_the_tool_reports_its_error_in_band() {
    let app = TestApp::for_module::<WithheldModule>()
        .await
        .expect("the hosts boot");
    let logs = LogCapture::install();

    let body = nest_rs_testing::mcp::call_tool(app.http(), KEPT, "in_band", None).await;
    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    assert!(!body.contains("eu-west"), "{body}");

    let line = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed");
    assert_eq!(line.level, "error");
    assert_eq!(line.field("path").as_deref(), Some(KEPT));
    assert_eq!(line.field("tool").as_deref(), Some("in_band"));
    assert_eq!(line.field("error").as_deref(), Some(LEDGER_REFUSED));
}

/// A filter answering `.opaque()`'s failure `503` files the chain `.opaque()`
/// kept at `warn`, the server unable to serve the call in time.
#[tokio::test]
async fn a_filter_answering_503_over_opaque_files_its_chain_at_warn() {
    let app = TestApp::for_module::<WithheldModule>()
        .await
        .expect("the hosts boot");
    let logs = LogCapture::install();

    let body = nest_rs_testing::mcp::call_tool(app.http(), KEPT, "opaqued", None).await;
    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(answer["error"]["data"]["code"], "UNAVAILABLE", "{answer}");

    let line = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation unavailable");
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("tool").as_deref(), Some("opaqued"));
    assert_eq!(line.field("error").as_deref(), Some(LEDGER_REFUSED));
    logs.expect_none(nest_rs_mcp::TARGET, "mcp operation failed");
}

/// A filter's answer speaks for the failure it answered alone: a chain
/// `.opaque()` kept for a failure the tool handled stays filed as it was, and
/// the failure the filter answers `503` files its own chain at `warn`.
#[tokio::test]
async fn a_filter_answering_another_failure_leaves_a_kept_chain_its_own() {
    let app = TestApp::for_module::<WithheldModule>()
        .await
        .expect("the hosts boot");
    let logs = LogCapture::install();

    let body = nest_rs_testing::mcp::call_tool(app.http(), KEPT, "fallback", None).await;
    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(answer["error"]["data"]["code"], "UNAVAILABLE", "{answer}");

    let answered = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation unavailable");
    assert_eq!(answered.level, "warn");
    assert_eq!(answered.field("tool").as_deref(), Some("fallback"));
    assert_eq!(answered.field("error").as_deref(), Some(STORE_REFUSED));
    let kept = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed");
    assert_eq!(kept.level, "error");
    assert_eq!(kept.field("tool").as_deref(), Some("fallback"));
    assert_eq!(kept.field("error").as_deref(), Some(LEDGER_REFUSED));
}
