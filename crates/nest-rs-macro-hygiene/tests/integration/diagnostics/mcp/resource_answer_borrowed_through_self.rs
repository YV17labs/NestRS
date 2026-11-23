//! What a resource read answers is read by its type outside the method, where
//! the method's lifetimes are undeclared, so the answer borrows nothing of the
//! method: `Json<&'a str>` borrowed through `&'a self` is rustc's `E0261`, the
//! lifetime deliberately failing to resolve where its type is read.

use nest_rs::mcp::{Json, McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost {
    title: String,
}

#[tools]
impl DemoHost {
    /// One note's title.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note<'a>(&'a self, id: u32) -> Result<Json<&'a str>, McpError> {
        let _ = id;
        Ok(Json(&self.title))
    }
}

fn main() {}
