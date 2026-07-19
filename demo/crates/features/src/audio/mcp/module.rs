use nest_rs_core::module;

use super::tool::AudioTool;
use crate::audio::AudioModule;
use crate::authz::mcp::AuthzMcpModule;

// The MCP edge composes like every other adapter: the port (which owns the
// storage import) plus the transport's authz bridge — importing this module is
// what makes the assistant's `/mcp` JWT-guarded instead of deny-all.
#[module(
    imports = [AudioModule, AuthzMcpModule],
    providers = [AudioTool],
)]
pub struct AudioMcpModule;
