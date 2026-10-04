//! A masked operation spells its return `Result<…>`: the mask reads the value's
//! shape — `Json<T>` to unwrap, `CallToolResult` to refuse — off that spelling,
//! and a `Result` renamed on import hides it. Refused at the return type, with
//! both remedies.

use nest_rs::mcp::{Json, McpError, mcp, tools};

type Answered<T> = Result<T, McpError>;

struct Read;
mod users {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a row.")]
    #[authorize(Read, users::Entity)]
    async fn row(&self) -> Answered<Json<String>> {
        Ok(Json("ok".into()))
    }
}

fn main() {}
