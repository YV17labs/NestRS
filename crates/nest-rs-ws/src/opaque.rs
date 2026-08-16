//! What a failing message tells the client, and what it tells the operator — the
//! WS half of the seam MCP established.
//!
//! A message's reply is built from whatever the handler returned, and its *error*
//! frame from whatever the handler's error type prints. `Display` is the wrong
//! default for that: a `DbErr` carries SQL, column names and sometimes row values.
//! The framework's own `ServiceError::Db` is `#[error("database error")]` so
//! nothing leaks today, but a feature's own error type has no such discipline
//! imposed on it — and a WS client is exactly as untrusted as a language model.
//!
//! ```ignore
//! #[subscribe_message("rooms.list")]
//! #[authorize(Read, rooms::Entity)]
//! async fn rooms(&self) -> Result<Vec<Room>, WsError> {
//!     Ok(self.svc.list().await.opaque()?)
//! }
//! ```
//!
//! **A deliberate error is not this.** A validation rejection, a `Denial`, a
//! message a client can act on — those exist to be *read*. Return them directly.

use std::fmt::Display;

use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

use crate::envelope::WsError;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error is printable, so it covers a
/// `DbErr`, a storage error, an `anyhow::Error` and a feature's own type without
/// any of them having to know WebSockets exist.
///
/// The twin traits on MCP and GraphQL are deliberately separate types rather than
/// one trait generic over the error: the output is what lets `.opaque()?` infer
/// from the enclosing handler's return type. See `nest_rs_core::opaque`.
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the client an opaque one.
    fn opaque(self) -> Result<T, WsError>;
}

impl<T, E: Display> Opaque<T> for Result<T, E> {
    fn opaque(self) -> Result<T, WsError> {
        self.map_err(|err| {
            tracing::error!(
                target: "nest_rs::ws",
                error = %err,
                "websocket message failed",
            );
            WsError::new(OPAQUE_CLIENT_MESSAGE)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error whose `Display` carries exactly what must not ship.
    struct Leaky;

    impl Display for Leaky {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("SELECT password_hash FROM \"user\" WHERE email = 'a@b.test'")
        }
    }

    #[test]
    fn the_frame_carries_a_constant_not_the_error() {
        let out: Result<(), WsError> = Err(Leaky).opaque();
        let err = out.expect_err("the failure stays a failure");
        assert_eq!(err.error, OPAQUE_CLIENT_MESSAGE);
        assert!(
            !err.error.contains("password_hash"),
            "the whole point: a `Display` carrying SQL does not reach the wire",
        );
    }

    #[test]
    fn the_structured_member_is_left_empty() {
        let out: Result<(), WsError> = Err(Leaky).opaque();
        assert!(
            out.expect_err("a failure").errors.is_none(),
            "an opaque failure has no per-field detail to offer — that member is \
             for rejections a client can act on",
        );
    }

    #[test]
    fn a_success_passes_through_untouched() {
        let out: Result<i32, WsError> = Ok::<_, Leaky>(7).opaque();
        assert_eq!(out.ok(), Some(7));
    }

    /// The other half of the same contract, and the half nobody read.
    ///
    /// Withholding the cause from the client is only safe because it is
    /// recorded somewhere else — here, an error frame on a long-lived socket. If this event ever stopped
    /// carrying `error`, every `.opaque()?` on this edge would turn a real
    /// failure into a blank refusal with no trace at all, and the
    /// nothing-leaks test next door would still pass: it only asserts what is
    /// *absent* from the wire.
    #[test]
    fn and_the_operator_gets_the_error_the_client_does_not() {
        let logs = nest_rs_testing::LogCapture::install();
        let _ = Err::<(), _>(Leaky).opaque();

        let event = logs.expect_one("nest_rs::ws", "websocket message failed");
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("password_hash")),
            "the cause the frame withholds is exactly what the log has to carry, got {:?}",
            event.fields,
        );
    }
}
