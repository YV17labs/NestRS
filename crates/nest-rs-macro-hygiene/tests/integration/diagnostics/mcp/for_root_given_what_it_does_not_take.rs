//! `McpModule::for_root` given a value it takes no such declaration in says what
//! it takes, naming no conversion: the setup has none.

use nest_rs::core::module;
use nest_rs::mcp::McpModule;

#[module(imports = [McpModule::for_root("assistant")])]
struct AppModule;

fn main() {}
