use nest_rs::core::module;
use nest_rs::seaorm::ws::WsDataContext;
use nest_rs::ws::{SocketContext, WsModule};

use crate::app_authz::http::AppAuthzHttpModule;

#[module(
    imports = [AppAuthzHttpModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AppAuthzWsModule;
