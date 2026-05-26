use std::sync::Arc;

use async_trait::async_trait;
use nestrs_core::injectable;
use serde::Deserialize;
use thiserror::Error;

use crate::weather::config::WeatherConfig;
use crate::weather::dto::WeatherReport;

#[derive(Debug, Error)]
pub enum WeatherError {
    #[error("upstream weather provider returned an error: {0}")]
    Upstream(#[from] reqwest::Error),

    #[error("upstream weather provider returned no current_weather payload")]
    MissingPayload,
}

#[async_trait]
pub(crate) trait WeatherProvider: Send + Sync + 'static {
    async fn current(&self, latitude: f64, longitude: f64) -> Result<WeatherReport, WeatherError>;
}

#[injectable]
pub(in crate::weather) struct OpenMeteoClient {
    #[inject]
    http: Arc<reqwest::Client>,
    #[inject]
    config: Arc<WeatherConfig>,
}

#[async_trait]
impl WeatherProvider for OpenMeteoClient {
    async fn current(&self, latitude: f64, longitude: f64) -> Result<WeatherReport, WeatherError> {
        let url = format!(
            "{}?latitude={latitude}&longitude={longitude}&current_weather=true",
            self.config.base_url
        );
        let payload: OpenMeteoResponse = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let current = payload
            .current_weather
            .ok_or(WeatherError::MissingPayload)?;

        Ok(WeatherReport {
            temperature_c: current.temperature,
            wind_speed_kmh: current.windspeed,
            wind_direction_deg: current.winddirection,
            weather_code: current.weathercode,
            observed_at: current.time,
        })
    }
}

#[derive(Deserialize)]
struct OpenMeteoResponse {
    current_weather: Option<CurrentWeather>,
}

#[derive(Deserialize)]
struct CurrentWeather {
    temperature: f64,
    windspeed: f64,
    winddirection: f64,
    weathercode: u16,
    time: String,
}
