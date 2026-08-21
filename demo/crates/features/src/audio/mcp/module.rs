use nest_rs::core::module;

use super::tool::AudioTool;
use crate::app_authz::mcp::AppAuthzMcpModule;
use crate::audio::AudioModule;

#[module(
    imports = [AudioModule, AppAuthzMcpModule],
    providers = [AudioTool],
)]
pub struct AudioMcpModule;
