use thiserror::Error;

#[derive(Debug, Error)]
pub enum WeatherError {
    #[error("upstream weather provider returned an error: {0}")]
    Upstream(#[from] reqwest::Error),

    #[error("upstream weather provider returned no current_weather payload")]
    MissingPayload,
}
