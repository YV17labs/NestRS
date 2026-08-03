use nest_rs::core::module;
use nest_rs::http::{HttpConfig, HttpModule};

use crate::controller::HelloController;
use crate::service::HelloService;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 3100, ..Default::default() }),
    ],
    providers = [HelloService, HelloController],
)]
pub struct SutModule;
