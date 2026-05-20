use nestrs_core::module;
use nestrs_health::HealthModule;

use crate::users::UsersModule;

#[module(imports = [UsersModule, HealthModule])]
pub struct AppModule;
