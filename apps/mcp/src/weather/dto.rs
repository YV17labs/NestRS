use nest_rs_mcp::schemars;
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Deserialize, schemars::JsonSchema, Validate)]
pub struct CoordsParams {
    #[validate(range(min = -90.0, max = 90.0))]
    pub latitude: f64,

    #[validate(range(min = -180.0, max = 180.0))]
    pub longitude: f64,
}

#[derive(Debug)]
pub struct WeatherReport {
    pub temperature_c: f64,
    pub wind_speed_kmh: f64,
    pub wind_direction_deg: f64,
    pub weather_code: u16,
    pub observed_at: String,
}
