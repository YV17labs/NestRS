//! A prompt answers messages, prose the entity round-trip cannot read whatever
//! type carries them, so a gated `#[prompt]` says it is unmasked rather than
//! failing every fetch at runtime.

use nest_rs::mcp::model::GetPromptResult;
use nest_rs::mcp::{McpError, mcp, tools};

struct Read;
mod users {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// Draft a note to the users the caller may read.
    #[prompt]
    #[authorize(Read, users::Entity)]
    async fn draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(Vec::new()))
    }
}

fn main() {}
