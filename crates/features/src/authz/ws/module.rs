use nestrs_core::module;
use nestrs_database::ws::WsDataContext;
use nestrs_ws::{SocketContext, WsModule};

use super::guard::WsAuthGuard;
use crate::authz::http::AuthzHttpModule;

// Imports `WsModule` so the framework's `WsServer<Global>` registry comes
// along transitively — a feature's `<Feature>WsModule` then needs to list
// only this module, mirroring HTTP / GraphQL where one `Authz<Transport>Module`
// is the single import (the transport runtime is reached through it).
#[module(
    imports = [AuthzHttpModule, WsModule],
    providers = [
        WsDataContext as dyn SocketContext,
        WsAuthGuard,
    ],
)]
pub struct AuthzWsModule;
