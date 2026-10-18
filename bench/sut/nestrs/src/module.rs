use nest_rs::core::module;
use nest_rs::http::{HttpConfig, HttpModule};

use crate::controller::HelloController;
use crate::guard::BearerGuard;
use crate::interceptor::PassInterceptor;
use crate::service::HelloService;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 3100, ..Default::default() }),
    ],
    providers = [HelloService, BearerGuard, PassInterceptor, HelloController],
)]
pub struct SutModule;
