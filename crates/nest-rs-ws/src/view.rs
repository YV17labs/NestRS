//! [`WsView`], the WebSocket edge's view of the message it dispatches.

use std::sync::Arc;

use nest_rs_core::__private::{Withheld, is_same_failure, kept};
use nest_rs_core::{Failure, ServedAddress, Settled, UnitContext, UnitView};

use crate::envelope::{Rendered, WsReply};
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
/// #[derive(Layer)]
/// struct Maintenance;
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
    /// The path the message's gateway is mounted at; `None` for a message
    /// dispatched as a test calls it, which no gateway mounts.
    path: Option<Arc<str>>,
    data: Option<serde_json::Value>,
    reply: Option<WsReply>,
    /// What the reply withholds, filed for the operator once it goes out: set
    /// where the edge rendered a failure, dropped by a layer's own reply.
    withheld: Option<Withheld>,
    /// The failure the edge rendered, and whether it is the server's own,
    /// `None` until it rendered one: a layer's own reply changes what the
    /// client reads, never who failed, and a failure a layer raised in its
    /// place says it for itself.
    rendered: Option<(Failure, bool)>,
}

impl UnitView for WsView {
    fn served_address(&self) -> Option<ServedAddress<'_>> {
        let served = ServedAddress::event(self.event);
        Some(self.path.as_deref().map_or(served, |path| served.at(path)))
    }
}

impl WsView {
    /// The view of `event`'s message `data`, arrived on `client` at the
    /// gateway mounted at `path`, when one mounts it.
    pub(crate) fn new(
        client: WsClient,
        event: &'static str,
        path: Option<&Arc<str>>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            client,
            event,
            path: path.cloned(),
            data: Some(data),
            reply: None,
            withheld: None,
            rendered: None,
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

    /// The message's event, as its `#[subscribe_message]` declares it; the
    /// frame no handler reads is `_OTHER`, never what the client named.
    pub fn event(&self) -> &str {
        self.event
    }

    /// The message's `data`, until the handler takes it; `null` on a frame no
    /// envelope holds.
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
    /// in place of the failure's frame — the unit still fails and rolls back, and
    /// its span still says who failed. What an error frame keeps from its client
    /// (`.opaque()`'s chain) is filed once it is the answer.
    pub fn respond(&mut self, mut reply: WsReply) -> Settled {
        self.withheld = match &mut reply {
            WsReply::Error(frame) => frame.take_withheld(),
            WsReply::Reply(_) | WsReply::None => None,
        };
        self.reply = Some(reply);
        nest_rs_core::__private::settled()
    }

    /// Answer the message with the error frame `rendered` holds for `failure`,
    /// what it withholds kept for the moment the answer is final.
    pub(crate) fn answer_failed(&mut self, rendered: Rendered, failure: &Failure) -> Settled {
        let Rendered { frame, withheld } = rendered;
        self.rendered = Some((kept(failure), frame.server_failed()));
        let settled = self.respond(WsReply::Error(frame));
        self.withheld = withheld;
        settled
    }

    /// Whether `failure` is the server's own, when it is the failure the edge
    /// rendered; `None` for one it never rendered.
    pub(crate) fn server_failed(&self, failure: &Failure) -> Option<bool> {
        self.rendered
            .as_ref()
            .filter(|(rendered, _)| is_same_failure(rendered, failure))
            .map(|(_, server_failed)| *server_failed)
    }

    /// The answer, what it withholds, filed once it goes out, and the path of
    /// the gateway it goes out at, which that line names.
    pub(crate) fn into_answer(self) -> (Option<WsReply>, Option<Withheld>, Option<Arc<str>>) {
        (self.reply, self.withheld, self.path)
    }
}
