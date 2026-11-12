//! A parameter whose type's last segment is `Ambient` is what a guard attached, on
//! a tool as on a resolver or a message handler, written with its argument or
//! not: the tail resolves it as `nest_rs::core::Ambient`, so a type of the app's
//! own by that name does not compile, and rmcp never reads it as an argument.

use nest_rs::mcp::{McpError, mcp, tools};

struct Ambient;

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Say who is calling.")]
    #[public]
    async fn whoami(&self, caller: Ambient) -> Result<String, McpError> {
        let Ambient = caller;
        Ok("ada".to_owned())
    }
}

fn main() {}
