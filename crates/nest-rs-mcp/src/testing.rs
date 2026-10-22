//! Test doubles: the one answer every opaque failure gets.

use nest_rs_core::OPAQUE_CLIENT_MESSAGE;
use nest_rs_core::problem::code;
use rmcp::model::ErrorCode;

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
