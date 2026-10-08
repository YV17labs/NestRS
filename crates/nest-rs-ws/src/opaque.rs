//! What a failing message tells the client, and what it tells the operator: an
//! error frame prints the handler's error, and a `DbErr`'s `Display` carries SQL.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::injectable;
//! # use nest_rs_ws::{Gateway, Opaque, WsClient, WsError, WsReply, gateway, messages};
//! #
//! # #[derive(serde::Serialize)]
//! # struct Room {
//! #     name: String,
//! # }
//! #
//! # #[injectable]
//! # #[derive(Default)]
//! # struct RoomsService;
//! #
//! # impl RoomsService {
//! #     async fn list(&self) -> Result<Vec<Room>, std::io::Error> {
//! #         Err(std::io::Error::other("relation \"rooms\" does not exist"))
//! #     }
//! # }
//! #
//! # #[gateway(path = "/ws")]
//! # struct RoomsGateway {
//! #     #[inject]
//! #     svc: Arc<RoomsService>,
//! # }
//! #
//! # #[messages]
//! # impl RoomsGateway {
//! #[subscribe_message("rooms.list")]
//! #[public]
//! async fn rooms(&self) -> Result<Vec<Room>, WsError> {
//!     Ok(self.svc.list().await.opaque()?)
//! }
//! # }
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let gateway = RoomsGateway { svc: Arc::new(RoomsService) };
//! # let reply = gateway.dispatch(&WsClient::for_test(), "rooms.list", serde_json::Value::Null).await;
//!
//! assert!(matches!(reply, WsReply::Error(e) if e.error == nest_rs_core::OPAQUE_CLIENT_MESSAGE));
//! # Ok(())
//! # }
//! ```
//!
//! **A deliberate error is not this.** A validation rejection, a `Denial`, a
//! message a client can act on — those exist to be *read*. Return them directly.

use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

use crate::envelope::WsError;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error converts into a boxed error, so
/// the whole cause chain reaches the operator's line.
///
/// Separate from the MCP and GraphQL twins: the output type is what lets
/// `.opaque()?` infer from the handler's return type (see `nest_rs_core::opaque`).
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the client an opaque one.
    fn opaque(self) -> Result<T, WsError>;
}

impl<T, E> Opaque<T> for Result<T, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn opaque(self) -> Result<T, WsError> {
        self.map_err(|err| {
            let err = nest_rs_core::boxed_error(err);
            tracing::error!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(&*err),
                "websocket message failed",
            );
            WsError::new(OPAQUE_CLIENT_MESSAGE)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Display;

    use super::*;

    #[derive(Debug)]
    struct Leaky;

    impl std::error::Error for Leaky {}

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

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test reads the line the call logs, not its result"
    )]
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
