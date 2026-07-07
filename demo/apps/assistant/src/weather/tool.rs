use std::sync::Arc;

use nest_rs_mcp::mcp;
use nest_rs_mcp::{
    CallToolResult, Content, McpError, Parameters, ServerHandler, tool, tool_handler, tool_router,
};
use validator::Validate;

use crate::weather::coords::Coords;
use crate::weather::service::WeatherService;

#[mcp(path = "/mcp")]
#[derive(Clone)]
pub struct WeatherTool {
    #[inject]
    svc: Arc<dyn WeatherService>,
}

#[tool_router]
impl WeatherTool {
    #[tool(description = "Return the current weather at the given GPS coordinates (Open-Meteo).")]
    async fn current_weather(
        &self,
        Parameters(params): Parameters<Coords>,
    ) -> Result<CallToolResult, McpError> {
        params
            .validate()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let report = self
            .svc
            .current(params.latitude, params.longitude)
            .await
            .map_err(internal)?;

        let summary = format!(
            "{:.1}°C, wind {:.1} km/h @ {:.0}° (code {}, observed {})",
            report.temperature_c,
            report.wind_speed_kmh,
            report.wind_direction_deg,
            report.weather_code,
            report.observed_at,
        );

        Ok(CallToolResult::success(vec![Content::text(summary)]))
    }
}

fn internal(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

#[tool_handler]
impl ServerHandler for WeatherTool {}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::sync::Arc;

    use nest_rs_core::Discoverable;

    use super::WeatherTool;
    use crate::weather::service::WeatherService;

    #[test]
    fn mcp_tool_declares_its_injected_trait_dependency_for_the_access_graph() {
        assert!(WeatherTool::dependencies().is_empty());
        assert!(
            WeatherTool::injected().contains(&TypeId::of::<Arc<dyn WeatherService>>()),
            "the MCP tool's injected dyn WeatherService is recorded for the access graph",
        );
    }
}
