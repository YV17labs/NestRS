use nestrs_core::module;
use nestrs_health::HealthModule;

use crate::weather::WeatherModule;

#[module(imports = [WeatherModule, HealthModule])]
pub struct AppModule;
