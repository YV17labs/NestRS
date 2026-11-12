//! Test doubles: the one answer every opaque failure gets, and a session's
//! stream a test writes to or leaves unread.

use futures_util::Stream;
use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, code};
use rmcp::model::ErrorCode;
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use tokio::sync::mpsc::Receiver;

use crate::McpError;

/// That `error` is the opaque answer: JSON-RPC's internal error, the constant
/// sentence, and the `INTERNAL` code under `data`, as the class's problem
/// answers, and nothing else.
pub(crate) fn assert_opaque(error: &McpError) {
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR, "{error:?}");
    assert_eq!(error.message, OPAQUE_CLIENT_MESSAGE, "{error:?}");
    assert_eq!(
        error.data,
        Some(serde_json::json!({ "code": code::INTERNAL.as_str() })),
        "{error:?}",
    );
}

/// `receiver` as a session's stream, carrying what its sender writes.
pub(crate) fn stream_of(
    mut receiver: Receiver<ServerSseMessage>,
) -> impl Stream<Item = ServerSseMessage> + Send + 'static {
    futures_util::stream::poll_fn(move |cx| receiver.poll_recv(cx))
}
