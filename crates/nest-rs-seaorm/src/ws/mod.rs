//! WebSocket bridge (feature `ws`) — captures the connection's ambient
//! [`Executor`](crate::Executor) + ability once and re-installs them per
//! message dispatch.

mod context;

pub use context::WsDataContext;
