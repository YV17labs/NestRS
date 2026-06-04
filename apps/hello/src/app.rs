use nestrs_core::module;

use crate::controller::HelloController;
use crate::service::HelloService;

#[module(providers = [HelloService, HelloController])]
pub struct AppModule;
