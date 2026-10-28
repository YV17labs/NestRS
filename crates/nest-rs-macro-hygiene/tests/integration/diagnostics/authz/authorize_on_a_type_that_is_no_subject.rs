//! `#[authorize]` names an entity or a subject `#[subject]` declares; a plain
//! struct is neither, so the posture has no mask to arm and does not compile.

use nest_rs::authz::Manage;
use nest_rs::mcp::{McpError, mcp, tools};

struct Exporting;

#[mcp]
#[derive(Clone)]
struct ExportsTool;

#[tools]
impl ExportsTool {
    #[tool(description = "Start the export.")]
    #[authorize(Manage, Exporting)]
    async fn export(&self) -> Result<String, McpError> {
        Ok("queued".into())
    }
}

fn main() {}
