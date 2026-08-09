//! `#[mcp]` — the tool host mounts on the HTTP transport, so its expansion
//! names the container, the endpoint meta and the mount resolution. None of
//! those is a crate a tool host declares.
//!
//! The sharper half of the witness is what is **not** written below. rmcp's own
//! `#[tool_router]` / `#[tool_handler]` / `#[prompt]` family expands to bare
//! `rmcp::` paths resolved against the *call site*, which used to force a
//! `use nest_rs::mcp::rmcp;` into every host file. `#[mcp]` on the impl now
//! emits those inside a private module that carries the import itself, so this
//! file names neither `rmcp` nor `ServerHandler` nor a router — and if that ever
//! regresses, this crate needs a second dependency and stops compiling, which is
//! the whole point of it.

use nest_rs::mcp::model::{GetPromptResult, PromptMessage, Role};
use nest_rs::mcp::{McpError, Parameters, input, mcp};

/// The typed input a tool takes. `#[input]` carries the `serde` and `schemars`
/// derives with their `crate = ` overrides, so this file declares neither.
#[input]
pub struct HygieneArgs {
    /// Echoed straight back — the payload is irrelevant, the derives are not.
    pub value: String,
}

/// A host serving both halves of the decorator surface: tools and prompts.
#[mcp(path = "/hygiene")]
#[derive(Clone, Default)]
pub struct HygieneTool;

/// One authored block feeds both of rmcp's routers.
#[mcp]
impl HygieneTool {
    /// Echo the argument back.
    #[tool]
    async fn echo(&self, Parameters(args): Parameters<HygieneArgs>) -> Result<String, McpError> {
        Ok(args.value)
    }

    /// A prompt with no arguments.
    #[prompt]
    async fn greet(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "hello",
        )]))
    }
}
