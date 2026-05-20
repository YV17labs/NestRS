use nestrs_core::module;

use crate::weather::service::{OpenMeteoClient, WeatherProvider};

#[module(providers = [OpenMeteoClient as dyn WeatherProvider])]
pub struct WeatherModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_open_meteo_as_default_provider() {
        let container = WeatherModule::register(Container::builder()).build();
        let provider: Option<Arc<dyn WeatherProvider>> = container.get_dyn();
        assert!(provider.is_some());
    }
}
