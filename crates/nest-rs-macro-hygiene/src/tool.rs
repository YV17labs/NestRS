//! `#[mcp]` + `#[tools]`, through the umbrella alone: the layers a host and
//! an operation bind, a `Scoped<T>` parameter, and an error of the domain's
//! own type.
//!
//! rmcp's `#[tool_router]`/`#[tool_handler]`/`#[prompt]` expand to bare `rmcp::`
//! paths resolved at the call site; this file names no `rmcp`, so it stops
//! compiling if `#[tools]` ever stops carrying that import itself.

use nest_rs::core::{Layer, UnitContext};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::mcp::model::{GetPromptResult, PromptMessage, Role};
use nest_rs::mcp::{McpError, McpView, Parameters, Scoped, Valid, input, mcp, tools};

use crate::pipeline::{HygieneUnitAudit, HygieneUnitRefusal};

/// The typed input a tool takes, validated through `Valid<HygieneArgs>`.
#[input]
pub struct HygieneArgs {
    #[validate(length(min = 1))]
    pub value: String,
}

/// A guard bound per operation, so the expansion's chain call resolves.
#[nest_rs::core::injectable]
pub struct HygieneGuard;

impl Layer for HygieneGuard {}

#[async_trait]
impl Guard for HygieneGuard {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<McpView> for HygieneGuard {}

const STAMP: &str = "Answer with this sentence.";

/// A value `#[meta]` declares, read by type.
pub struct HygieneLimit(pub u32);

/// Built once per operation's request, read through a `Scoped<T>` parameter.
#[nest_rs::core::injectable(scope = request)]
#[derive(Default)]
pub struct HygieneBasket;

/// What a domain answers when it refuses: no MCP type names it.
#[derive(Debug)]
pub struct HygieneRefused;

impl std::fmt::Display for HygieneRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for HygieneRefused {}

/// Tools and prompts, under a host-scope guard, interceptor and exception
/// filter.
#[mcp(path = "/hygiene")]
#[meta(HygieneLimit(100))]
#[use_guards(HygieneGuard)]
#[use_interceptors(HygieneUnitAudit)]
#[use_exception_filters(HygieneUnitRefusal)]
#[derive(Clone, Default)]
pub struct HygieneTool;

/// A description stated three ways: an argument (`echo`), the doc comment
/// (`greet`) and a constant (`stamp`).
#[tools]
impl HygieneTool {
    #[tool(description = "Echo the argument back.")]
    #[public]
    #[meta(HygieneLimit(10))]
    async fn echo(
        &self,
        Parameters(args): Parameters<Valid<HygieneArgs>>,
    ) -> Result<String, McpError> {
        Ok(args.into_inner().value)
    }

    /// A prompt with no arguments.
    #[prompt]
    #[public]
    #[use_guards(HygieneGuard)]
    async fn greet(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "hello",
        )]))
    }

    #[tool(description = STAMP)]
    #[public]
    fn stamp(&self) -> Result<String, McpError> {
        Ok(STAMP.into())
    }

    #[tool(description = "Answer at once.")]
    #[public]
    fn ping(&self) -> Result<String, McpError> {
        Ok("pong".into())
    }

    #[tool(description = "Read this request's basket.")]
    #[public]
    #[use_interceptors(HygieneUnitAudit)]
    #[use_exception_filters(HygieneUnitRefusal)]
    async fn basket(&self, basket: Scoped<HygieneBasket>) -> Result<String, HygieneRefused> {
        let _ = &*basket;
        Err(HygieneRefused)
    }

    #[cfg(feature = "seaorm")]
    #[tool(description = "Count what the caller may read.")]
    #[authorize(nest_rs::authz::Read, crate::entity::Entity)]
    fn count(&self) -> Result<String, McpError> {
        Ok("0".into())
    }

    /// An operation compiled out takes its wrapper, route and guard with it.
    #[cfg(any())]
    #[tool(description = "Not in this build.")]
    #[public]
    #[use_guards(crate::does_not_exist::Guard)]
    async fn compiled_out(&self, input: crate::does_not_exist::Input) -> Result<String, McpError> {
        crate::does_not_exist::answer(input)
    }

    #[cfg_attr(all(), cfg(any()))]
    #[tool(description = "Not in this build either.")]
    #[public]
    fn compiled_out_by_cfg_attr(&self) -> Result<crate::does_not_exist::Output, McpError> {
        crate::does_not_exist::answer()
    }

    /// A duplicate tool name under an excluding condition is not a duplicate route.
    #[cfg(any())]
    #[tool(name = "ping", description = "Answer at once, elsewhere.")]
    #[public]
    fn ping_elsewhere(&self) -> Result<String, McpError> {
        Ok("pong".into())
    }
}
