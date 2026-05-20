use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_mcp::{
    tool, tool_handler, tool_router, CallToolResult, Content, McpError, Parameters, ServerHandler,
};
use validator::Validate;

use crate::weather::dto::CoordsParams;
use crate::weather::service::WeatherProvider;

#[injectable]
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
