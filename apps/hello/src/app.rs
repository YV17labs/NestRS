use nestrs_core::module;
use nestrs_http::HttpModule;

use crate::controller::HelloController;
use crate::service::HelloService;

#[module(
    imports = [HttpModule::for_root(None)],
    providers = [HelloService, HelloController],
)]
pub struct AppModule;
