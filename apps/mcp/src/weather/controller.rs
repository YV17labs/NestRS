use std::sync::Arc;

use nestrs_mcp::mcp;
use nestrs_mcp::{
    tool, tool_handler, tool_router, CallToolResult, Content, McpError, Parameters, ServerHandler,
};
use validator::Validate;

use crate::weather::dto::CoordsParams;
use crate::weather::service::WeatherProvider;

#[mcp(path = "/mcp")]
#[derive(Clone)]
pub struct WeatherController {
    #[inject]
    weather: Arc<dyn WeatherProvider>,
}

#[tool_router]
impl WeatherController {
    #[tool(description = "Return the current weather at the given GPS coordinates (Open-Meteo).")]
    async fn current_weather(
        &self,
        Parameters(params): Parameters<CoordsParams>,
    ) -> Result<CallToolResult, McpError> {
        params
            .validate()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let report = self
            .weather
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
impl ServerHandler for WeatherController {}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::sync::Arc;

    use nestrs_core::Discoverable;

    use super::WeatherController;
    use crate::weather::service::WeatherProvider;

    #[test]
    fn mcp_tool_declares_its_injected_trait_dependency_for_the_access_graph() {
        // An MCP tool is built per session, so `dependencies` is empty; `injected`
        // reports the `Arc<dyn WeatherProvider>` it pulls — keyed exactly as the
        // `provide_dyn` binding — so the access-graph check governs it.
        assert!(WeatherController::dependencies().is_empty());
        assert!(
            WeatherController::injected().contains(&TypeId::of::<Arc<dyn WeatherProvider>>()),
            "the MCP tool's injected dyn WeatherProvider is recorded for the access graph",
        );
    }
}
