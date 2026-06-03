use nestrs_core::module;
use nestrs_database::ws::WsDataContext;
use nestrs_ws::{SocketContext, WsModule};

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
