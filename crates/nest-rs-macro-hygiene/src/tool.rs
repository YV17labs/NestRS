//! `#[mcp]` + `#[tools]` — the tool host mounts on the HTTP transport, so its
//! expansion names the container, the endpoint meta and the mount resolution.
//! None of those is a crate a tool host declares.
//!
//! It now also names the **request layers**: the per-operation guard chain
//! (`nest-rs-guards`) and the per-argument pipe carrier (`nest-rs-pipes`). Both
//! are unconditional in the expansion of a decorated operation, and neither
//! appears in this crate's manifest — which is the whole assertion.
//!
//! The sharper half of the witness is what is **not** written below. rmcp's own
//! `#[tool_router]` / `#[tool_handler]` / `#[prompt]` family expands to bare
//! `rmcp::` paths resolved against the *call site*, which used to force a
//! `use nest_rs::mcp::rmcp;` into every host file. `#[tools]` now emits those
//! inside a private module that carries the import itself, so this
//! file names neither `rmcp` nor `ServerHandler` nor a router — and if that ever
//! regresses, this crate needs a second dependency and stops compiling, which is
//! the whole point of it.
//!
//! `#[authorize(Action, Entity)]` is deliberately *not* witnessed here: it needs
//! a real entity, which is the same reason this crate does not consume
//! `#[crud]`/`#[expose]`. Its expansion is proved by `nest-rs-mcp`'s own suite
//! and by `demo/`.

use nest_rs::core::Layer;
use nest_rs::guards::{Denial, Guard, McpGuard, async_trait};
use nest_rs::mcp::model::{GetPromptResult, PromptMessage, Role};
use nest_rs::mcp::{McpError, McpOperationContext, Parameters, Valid, input, mcp, tools};

/// The typed input a tool takes. `#[input]` carries the `serde`, `schemars` and
/// `validator` derives with their `crate = ` overrides, so this file declares
/// none of them — and `Valid<HygieneArgs>` below is what makes the last one
/// load-bearing.
#[input]
pub struct HygieneArgs {
    /// Echoed straight back — the payload is irrelevant, the derives are not.
    #[validate(length(min = 1))]
    pub value: String,
}

/// A guard bound per operation, so the expansion's chain call has something real
/// to resolve. `#[injectable]` and the `Guard`/`Layer` pair both come from the
/// umbrella; a host binding a guard declares no layer crate of its own.
#[nest_rs::core::injectable]
pub struct HygieneGuard;

impl Layer for HygieneGuard {}

#[async_trait]
impl Guard for HygieneGuard {
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl McpGuard for HygieneGuard {}

/// A host serving both halves of the decorator surface: tools and prompts, with
/// a host-scope guard the way a controller or a resolver declares one.
#[mcp(path = "/hygiene")]
#[use_guards(HygieneGuard)]
#[derive(Clone, Default)]
pub struct HygieneTool;

/// One authored block feeds both of rmcp's routers.
///
/// The two operations also witness the two ways a description is stated: `echo`
/// declares it as an argument — the form `demo/` and every scaffold use, and the
/// only one available to a codebase that carries no comments — and `greet` lets
/// the doc comment fall through. An operation stating neither does not compile.
#[tools]
impl HygieneTool {
    #[tool(description = "Echo the argument back.")]
    #[public]
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
}
