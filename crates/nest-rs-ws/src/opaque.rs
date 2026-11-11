//! A failure the client is owed no word of, said where it is met: `.opaque()`
//! turns any error into the [`WsError`] a handler returns, answering
//! [`OPAQUE_CLIENT_MESSAGE`](nest_rs_core::OPAQUE_CLIENT_MESSAGE) under the
//! `INTERNAL` code, whatever the error's type would otherwise have answered,
//! and handing the gateway the whole chain, which it files at `error` naming
//! the message once that frame is its answer.
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

use nest_rs_core::__private::Withheld;

use crate::envelope::WsError;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error converts into a boxed error, so
/// the whole cause chain reaches the operator's line.
///
/// Separate from the MCP and GraphQL twins: the output type is what lets
/// `.opaque()?` infer from the handler's return type (see `nest_rs_core::opaque`).
pub trait Opaque<T> {
    /// Hand the client an opaque error, keeping the real one for the
    /// operator's line, which the gateway files once it is the answer.
    fn opaque(self) -> Result<T, WsError>;
}

impl<T, E> Opaque<T> for Result<T, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn opaque(self) -> Result<T, WsError> {
        self.map_err(|err| {
            let said = nest_rs_core::error_message(&*nest_rs_core::boxed_error(err));
            WsError::internal().withholding(Withheld::Failed(said))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Display;

    use nest_rs_core::OPAQUE_CLIENT_MESSAGE;

    use super::*;
    use crate::envelope::WsReply;

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

    /// No per-field detail — that is for rejections a client can act on —
    /// and the class a client branches on, as every other edge names it.
    #[test]
    fn the_structured_member_names_the_internal_class_alone() {
        let out: Result<(), WsError> = Err(Leaky).opaque();
        crate::testing::assert_opaque(&out.expect_err("a failure"));
    }

    #[test]
    fn a_success_passes_through_untouched() {
        let out: Result<i32, WsError> = Ok::<_, Leaky>(7).opaque();
        assert_eq!(out.ok(), Some(7));
    }

    /// The chain is handed to the gateway, which files it once the frame is
    /// the answer: nothing is filed at the call, and the frame keeps it off the
    /// wire.
    #[test]
    fn and_the_operator_gets_the_error_the_client_does_not() {
        let logs = nest_rs_testing::LogCapture::install();
        let err = Err::<(), _>(Leaky).opaque().expect_err("a failure");

        logs.expect_none("nest_rs::ws", "websocket message failed");
        assert!(
            matches!(
                err.server_failure.as_deref().and_then(|failure| failure.withheld.as_ref()),
                Some(Withheld::Failed(said)) if said.contains("password_hash")
            ),
            "the cause the frame withholds is exactly what the line has to carry, got {:?}",
            err.server_failure,
        );
        let wire = serde_json::to_string(&err).expect("serializes");
        assert!(!wire.contains("password_hash"), "{wire}");

        let WsReply::Error(_) =
            crate::envelope::chain_rendered(&err).into_reply(Some("rooms.list"))
        else {
            panic!("an error answers an error frame");
        };
        let event = logs.expect_one("nest_rs::ws", "websocket message failed");
        assert_eq!(event.level, "error");
        assert_eq!(event.field("event").as_deref(), Some("rooms.list"));
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("password_hash")),
            "{:?}",
            event.fields,
        );
    }
}
