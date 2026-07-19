use nest_rs_core::module;

use super::tool::AudioTool;
use crate::audio::AudioModule;
use crate::authz::mcp::AuthzMcpModule;

#[module(
    imports = [AudioModule, AuthzMcpModule],
    providers = [AudioTool],
)]
pub struct AudioMcpModule;
