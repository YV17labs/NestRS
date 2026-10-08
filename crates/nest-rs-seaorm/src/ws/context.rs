//! The ability is captured once at the upgrade and does not see a mid-connection
//! revocation, logout or token expiry; `WsConfig::max_connection` bounds that
//! window. The guard chain is not re-run per message.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_ws::{BoxFuture, Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::dispatch::{RequestSnapshot, with_data_context};

/// Re-installs the data context for a gateway's message handlers. List `as dyn
/// SocketContext` on the gateway's module.
#[injectable]
pub struct WsDataContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl SocketContext for WsDataContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(RequestSnapshot::capture(&self.db, req))
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply> {
        Box::pin(with_data_context(
            captured,
            "ws",
            inner,
            |reply| !matches!(reply, WsReply::Error(_)),
            || WsReply::error(nest_rs_core::OPAQUE_CLIENT_MESSAGE),
        ))
    }
}
