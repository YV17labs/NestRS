use nest_rs::core::module;
use nest_rs::seaorm::ws::WsDataContext;
use nest_rs::ws::{SocketContext, WsModule};

use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AuthzWsModule;
