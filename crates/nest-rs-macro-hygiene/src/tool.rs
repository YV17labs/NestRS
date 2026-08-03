//! `#[mcp]` — the tool host mounts on the HTTP transport, so its expansion
//! names the container, the endpoint meta and the mount resolution. None of
//! those is a crate a tool host declares.
//!
//! rmcp's own `#[tool_router]` / `#[tool_handler]` / `#[tool]` are exercised
//! here too, and that is the sharper half of the witness: they expand to bare
//! `rmcp::` paths resolved against **this module's scope**, so `use
//! nest_rs::mcp::rmcp;` is what supplies the name. If that re-export ever stops
//! covering them, this crate needs a second dependency and stops compiling —
//! which is the whole point of the crate.

// The name rmcp's macro expansions resolve against. One `use`, no manifest line.
use nest_rs::mcp::model::{GetPromptResult, PromptMessage, Role};
use nest_rs::mcp::rmcp;
use nest_rs::mcp::{
    CallToolResult, ContentBlock, McpError, Parameters, ServerHandler, input, mcp, prompt,
    prompt_handler, prompt_router, tool, tool_handler, tool_router,
};

/// The typed input a tool takes. `#[input]` carries the `serde` and `schemars`
/// derives with their `crate = ` overrides, so this file declares neither.
#[input]
pub struct HygieneArgs {
    /// Echoed straight back — the payload is irrelevant, the derives are not.
    pub value: String,
}

/// A host serving both halves of the decorator surface: tools and prompts.
#[mcp(path = "/hygiene-mcp")]
#[derive(Clone, Default)]
pub struct HygieneTool;

#[tool_router]
impl HygieneTool {
    #[tool(description = "Echo the argument back.")]
    async fn echo(
        &self,
        Parameters(args): Parameters<HygieneArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            args.value,
        )]))
    }
}

#[prompt_router]
impl HygieneTool {
    #[prompt(description = "A prompt with no arguments.")]
    async fn greet(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "hello",
        )]))
    }
}

// Stacked on one `impl ServerHandler`, which is how rmcp composes a host that
// serves more than one capability.
#[tool_handler]
#[prompt_handler]
impl ServerHandler for HygieneTool {}
