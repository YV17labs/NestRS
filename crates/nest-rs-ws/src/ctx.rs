//! WebSocket's reading of `nest_rs_core::Ctx<T>`: a `#[subscribe_message]`
//! parameter reading what a guard attached — the upgrade's guards for every
//! message of the socket, the message's own for its handler.
//!
//! ```
//! use nest_rs_core::{AmbientValue, Ctx, Layer, UnitContext, injectable};
//! use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
//! use nest_rs_http::HttpView;
//! use nest_rs_ws::{gateway, messages};
//! # use nest_rs_core::module;
//! # use nest_rs_ws::WsModule;
//!
//! #[derive(Clone)]
//! struct Caller(String);
//!
//! impl AmbientValue for Caller {}
//!
//! /// Attaches the caller at the upgrade, the way an authentication guard does.
//! #[injectable]
//! #[derive(Layer)]
//! struct NameGuard;
//!
//! #[async_trait]
//! impl Guard for NameGuard {
//!     async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         let name = HttpView::of(cx)?.request().header("x-name").unwrap_or("anonymous");
//!         let caller = Caller(name.to_owned());
//!         cx.attach(caller);
//!         Ok(())
//!     }
//! }
//!
//! impl GuardFor<HttpView> for NameGuard {}
//!
//! #[gateway(path = "/ws")]
//! #[use_guards(NameGuard)]
//! #[derive(Default)]
//! struct WhoGateway;
//!
//! #[messages]
//! impl WhoGateway {
//!     #[subscribe_message("whoami")]
//!     #[public]
//!     async fn whoami(&self, caller: Ctx<Caller>) -> String {
//!         caller.into_inner().0
//!     }
//! }
//! #
//! # #[module(imports = [WsModule], providers = [NameGuard, WhoGateway])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::builder().module::<AppModule>().build_ws().await?;
//! let mut socket = app.socket("/ws").header("x-name", "ada").connect().await;
//! socket.send("whoami", serde_json::Value::Null).await;
//!
//! assert_eq!(socket.next_envelope().await["data"], "ada");
//! # app.shutdown().await?;
//! # Ok(())
//! # }
//! ```

use nest_rs_core::{AmbientValue, Ctx};

use crate::envelope::WsReply;

/// The `Ctx<T>` a message's handler takes, read once its guards passed; a miss
/// answers `event` the opaque frame, the type filed at `error`, never answered.
pub fn ctx_parameter<T: AmbientValue + Clone>(event: &str) -> Result<Ctx<T>, WsReply> {
    nest_rs_core::__private::attached::<T>()
        .map_err(|missing| WsReply::from_handler_error(event, missing))
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    #[derive(Clone)]
    struct Hidden;

    impl AmbientValue for Hidden {}

    #[test]
    fn a_miss_answers_opaquely_and_files_the_type() {
        let logs = LogCapture::install();

        let Err(WsReply::Error(frame)) = ctx_parameter::<Hidden>("say") else {
            panic!("a miss answers an error frame");
        };

        let said = serde_json::to_string(&frame).expect("serializes");
        assert!(!said.contains("Hidden"), "the frame names the type: {said}");
        crate::testing::assert_opaque(&frame);
        let line = logs.expect_one(crate::TARGET, "websocket message failed");
        assert!(
            line.field("error")
                .is_some_and(|error| error.contains("ctx::tests::Hidden")),
            "{:?}",
            line.fields,
        );
    }
}
