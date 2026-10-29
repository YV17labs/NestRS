//! A template is read back off the URI a client asks for, which only a level-1
//! expression allows: an RFC 6570 operator is refused at the template, with its
//! site first.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// A file, by its path.
    #[resource(template = "file://{+path}")]
    #[public]
    async fn file(&self, path: String) -> Result<String, McpError> {
        Ok(path)
    }
}

fn main() {}
