//! Each operation of a `#[tools]` impl declares its site in a `static`, which no
//! generic parameter of the impl reaches: a generic host has no one site per
//! operation, so it is refused where it is written.

use nest_rs::mcp::{McpError, tools};

#[derive(Clone, Default)]
struct DemoTool<T>(std::marker::PhantomData<T>);

#[tools]
impl<T: Send + Sync + 'static> DemoTool<T> {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
