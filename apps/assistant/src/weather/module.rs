use nest_rs_core::module;
use nest_rs_mcp::{AllowAllMcpGuard, McpOperationGuard};

use crate::weather::service::{OpenMeteoClient, WeatherProvider};
use crate::weather::tool::WeatherTool;

#[module(providers = [
    OpenMeteoClient as dyn WeatherProvider,
    AllowAllMcpGuard as dyn McpOperationGuard,
    WeatherTool,
])]
pub struct WeatherModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_open_meteo_as_default_provider() {
        let container = WeatherModule::register(Container::builder()).build();
        let provider: Option<Arc<dyn WeatherProvider>> = container.get_dyn();
        assert!(provider.is_some());
    }
}
