//! WebSocket bridge (feature `ws`): re-installs the connection's ambient
//! [`Executor`](crate::Executor) and ability per message dispatch.

mod context;

pub use context::WsDataContext;
