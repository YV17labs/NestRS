#[derive(Debug)]
pub struct WeatherReport {
    pub temperature_c: f64,
    pub wind_speed_kmh: f64,
    pub wind_direction_deg: f64,
    pub weather_code: u16,
    pub observed_at: String,
}
