use nest_rs::core::module;
use nest_rs::mcp::McpModule;

use super::tool::AudioTool;
use crate::audio::AudioModule;
use crate::authz::mcp::AuthzMcpModule;

#[module(
    imports = [AudioModule, AuthzMcpModule, McpModule],
    providers = [AudioTool],
)]
pub struct AudioMcpModule;
