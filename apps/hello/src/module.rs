use nest_rs_core::module;
use nest_rs_http::HttpModule;

use crate::controller::HelloController;
use crate::service::HelloService;

#[module(
    imports = [HttpModule::for_root(None)],
    providers = [HelloService, HelloController],
)]
pub struct HelloModule;
