//! A tool reads `Ambient<T>` by its name and leaves it out of the arguments its
//! schema describes. Behind an alias it would be an argument the model sends, so
//! it is refused naming the fix, beside rmcp's own refusal of the tool.

use nest_rs::core::{Ambient, AmbientValue};
use nest_rs::mcp::{McpError, mcp, tools};

#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

type Me = Ambient<Caller>;

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Say who is calling.")]
    #[public]
    async fn whoami(&self, _caller: Me) -> Result<String, McpError> {
        Ok(String::new())
    }
}

fn main() {}
