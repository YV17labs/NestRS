use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::tool::AudioTool;
use crate::audio::AudioModule;
use crate::authz::mcp::AuthzMcpModule;

#[module(
    imports = [AudioModule, AuthzMcpModule, HttpModule],
    providers = [AudioTool],
)]
pub struct AudioMcpModule;
