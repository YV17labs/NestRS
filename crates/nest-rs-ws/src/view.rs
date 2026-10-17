//! [`WsView`], the WebSocket edge's view of the message it dispatches.

use nest_rs_core::UnitView;

use crate::server::WsClient;

/// The WebSocket edge's view of one message a guard checks: the connection it
/// arrived on, its event, and its `data` until the handler takes it.
///
/// A guard reaches it through `UnitContext::switch_to` in `Guard::check`;
/// deciding access from the payload is a pipe's job, one step from deciding it
/// *from* the payload.
pub struct WsView {
    client: WsClient,
    event: &'static str,
    data: Option<serde_json::Value>,
}

impl UnitView for WsView {}

impl WsView {
    /// The view of `event`'s message `data`, arrived on `client`.
    pub(crate) fn new(client: WsClient, event: &'static str, data: serde_json::Value) -> Self {
        Self {
            client,
            event,
            data: Some(data),
        }
    }

    /// The view of `event`'s message arrived on `client`, whose `data` the
    /// global pipes hold while they read it.
    pub(crate) fn piped(client: WsClient, event: &'static str) -> Self {
        Self {
            client,
            event,
            data: None,
        }
    }

    /// The message's `data` back, for the handler, once the checks are over.
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

    /// The message's `data`, until the global pipes or the handler take it.
    pub fn data(&self) -> Option<&serde_json::Value> {
        self.data.as_ref()
    }
}
