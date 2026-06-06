use nest_rs_core::module;
use nest_rs_seaorm::ws::WsDataContext;
use nest_rs_ws::{SocketContext, WsModule};

use super::guard::WsAuthGuard;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
        WsAuthGuard,
    ],
)]
pub struct AuthzWsModule;
