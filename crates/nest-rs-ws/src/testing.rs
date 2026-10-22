//! Test doubles: the one frame every opaque failure answers.

use nest_rs_core::OPAQUE_CLIENT_MESSAGE;
use nest_rs_core::problem::code;

use crate::envelope::WsError;

/// That `frame` is the opaque one: the constant sentence and the `INTERNAL`
/// code under `errors`, as the class's problem frame reads, and nothing else.
pub(crate) fn assert_opaque(frame: &WsError) {
    assert_eq!(
        serde_json::to_value(frame).expect("a frame serializes"),
        serde_json::json!({
            "error": OPAQUE_CLIENT_MESSAGE,
            "errors": { "code": code::INTERNAL.as_str() },
        }),
    );
}
