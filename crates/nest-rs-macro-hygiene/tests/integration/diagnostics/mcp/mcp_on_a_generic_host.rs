//! Each operation `#[tools]` declares on a host runs a `static` site, which no
//! parameter of the struct reaches: a generic `#[mcp]` struct has no one site
//! per operation, so it is refused where it is written.

use nest_rs::mcp::mcp;

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool<T>(std::marker::PhantomData<T>);

fn main() {}
