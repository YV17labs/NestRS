use std::time::Duration;

use async_trait::async_trait;
use nest_rs_core::injectable;
use serde::Deserialize;
use thiserror::Error;

use crate::weather::dto::WeatherReport;

const BASE_URL: &str = "https://api.open-meteo.com/v1/forecast";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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
#[derive(Default)]
pub(in crate::weather) struct OpenMeteoClient {
    http: reqwest::Client,
}

#[async_trait]
impl WeatherProvider for OpenMeteoClient {
    async fn current(&self, latitude: f64, longitude: f64) -> Result<WeatherReport, WeatherError> {
        tracing::debug!(target: "assistant::weather", latitude, longitude, "fetching current weather");
        let url =
            format!("{BASE_URL}?latitude={latitude}&longitude={longitude}&current_weather=true");
        let payload: OpenMeteoResponse = self
            .http
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let current = payload
            .current_weather
            .ok_or(WeatherError::MissingPayload)?;
        tracing::info!(
            target: "assistant::weather",
            latitude,
            longitude,
            temperature_c = current.temperature,
            "fetched current weather"
        );

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
