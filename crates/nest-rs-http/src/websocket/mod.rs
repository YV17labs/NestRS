//! The WebSocket edges' shared ground: whether a request asks for a WebSocket
//! and, behind the `websocket` feature, the opening handshake (RFC 6455 §4.2)
//! on hyper's upgrade, the socket over tokio-tungstenite, and the close codes
//! both a gateway and a graphql-ws socket end with.

#[cfg(feature = "websocket")]
mod close_code;
mod token;
#[cfg(feature = "websocket")]
mod upgrade;

#[cfg(feature = "websocket")]
pub use close_code::{CloseCode, close_frame, read_failed};
pub use token::requested;
#[cfg(feature = "websocket")]
pub use tokio_tungstenite::tungstenite::{Error, Message, Utf8Bytes};
#[cfg(feature = "websocket")]
pub use upgrade::{WebSocketLimits, WebSocketStream, WebSocketUpgrade};
