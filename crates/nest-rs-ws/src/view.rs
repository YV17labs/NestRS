//! [`WsView`], the WebSocket edge's view of the message it dispatches.

use nest_rs_core::{Failure, Settled, UnitContext, UnitView};

use crate::envelope::{Line, Rendered, WsReply};
use crate::server::WsClient;

/// The WebSocket edge's view of one message: the connection it arrived on,
/// its event, its `data` until the handler takes it, and what the message
/// answers.
///
/// A guard reaches it through `UnitContext::switch_to` in `Guard::check`;
/// deciding access from the payload is a pipe's job, one step from deciding it
/// *from* the payload. A layer answers the message without running the rest of
/// it through [`respond`](Self::respond):
///
/// ```
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
/// use nest_rs_ws::{WsReply, WsView};
///
/// struct Maintenance;
///
/// impl Layer for Maintenance {}
///
/// #[async_trait]
/// impl Interceptor for Maintenance {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         if let Some(message) = cx.switch_to_mut::<WsView>() {
///             return Ok(message.respond(WsReply::error("down for maintenance")));
///         }
///         next.run(cx).await
///     }
/// }
/// ```
pub struct WsView {
    client: WsClient,
    event: &'static str,
    data: Option<serde_json::Value>,
    reply: Option<WsReply>,
    /// What the reply files for the operator once it goes out: set where the
    /// edge rendered a failure, dropped by a layer's own reply.
    line: Option<Line>,
}

impl UnitView for WsView {}

impl WsView {
    /// The view of `event`'s message `data`, arrived on `client`.
    pub(crate) fn new(client: WsClient, event: &'static str, data: serde_json::Value) -> Self {
        Self {
            client,
            event,
            data: Some(data),
            reply: None,
            line: None,
        }
    }

    /// The view of the unit `cx` runs, which the edge built.
    pub(crate) fn of<'v>(cx: &'v mut UnitContext<'_>) -> Result<&'v mut Self, Failure> {
        cx.switch_to_mut::<Self>().ok_or_else(|| {
            Failure::new(nest_rs_core::anyhow::anyhow!(
                "a WebSocket message's unit runs without its view"
            ))
        })
    }

    /// The message's `data`, taken for the tail once the checks are over.
    pub(crate) fn take_data(&mut self) -> serde_json::Value {
        self.data.take().unwrap_or_default()
    }

    /// The connection the message arrived on.
    pub fn client(&self) -> &WsClient {
        &self.client
    }

    /// The message's event, as its `#[subscribe_message]` declares it.
    pub fn event(&self) -> &str {
        self.event
    }

    /// The message's `data`, until the handler takes it.
    pub fn data(&self) -> Option<&serde_json::Value> {
        self.data.as_ref()
    }

    /// What the message answers so far: `None` until its handler, or a layer,
    /// answered it.
    pub fn reply(&self) -> Option<&WsReply> {
        self.reply.as_ref()
    }

    /// Answer the message with `reply` in place of whatever it held: from an
    /// interceptor, without running the rest of it; from an exception filter,
    /// in place of the failure's frame — the unit still fails and rolls back.
    pub fn respond(&mut self, reply: WsReply) -> Settled {
        self.reply = Some(reply);
        self.line = None;
        nest_rs_core::__private::settled()
    }

    /// Answer the message with the error frame `rendered` holds, its line kept
    /// for the moment the answer is final.
    pub(crate) fn answer_failed(&mut self, rendered: Rendered) -> Settled {
        let Rendered { frame, line } = rendered;
        let settled = self.respond(WsReply::Error(frame));
        self.line = line;
        settled
    }

    /// The answer, and the line it files once it goes out.
    pub(crate) fn into_answer(self) -> (Option<WsReply>, Option<Line>) {
        (self.reply, self.line)
    }
}
