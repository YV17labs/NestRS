use nest_rs_core::module;
use nest_rs_http::{HttpConfig, HttpModule};

use crate::controller::HelloController;
use crate::service::HelloService;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 3100, ..Default::default() }),
    ],
    providers = [HelloService, HelloController],
)]
pub struct SutModule;
