//! Global pipes — JSON-body shaping that applies to every handler.
//!
//! A [`GlobalPipe`] inspects or rewrites the JSON request body **before** the
//! extractor runs. Registered once with
//! `App::builder().use_pipes_global(...)`, it applies to every JSON-bodied
//! HTTP handler in the app — no per-route opt-in. The framework gives
//! pipes the [`LayerKind::Pipe`](nest_rs_core::LayerKind) execution slot:
//! they run after Guards and before the handler. Per-route opt-out is
//! `#[no_pipes]`.
//!
//! ## Defining a global pipe
//!
//! ```rust,ignore
//! use nest_rs_core::{injectable, Layer};
//! use nest_rs_pipes::{GlobalPipe, PipeError};
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct StripUnknownFields;
//!
//! impl Layer for StripUnknownFields {}
//!
//! impl GlobalPipe for StripUnknownFields {
//!     fn transform_body(&self, value: &mut serde_json::Value) -> Result<(), PipeError> {
//!         // …
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Registering globally
//!
//! ```rust,ignore
//! use nest_rs_guards::{AppBuilderPipesExt, pipe};
//!
//! App::builder()
//!     .use_pipes_global([pipe::<StripUnknownFields>()])
//!     .module::<AppModule>()
//! ```

use std::sync::Arc;

use nest_rs_core::Layer;

use crate::pipe::PipeError;

/// A request-body validator/transformer applied to every handler across
/// every transport. Runs after [`Guard`]s, before the extractor (HTTP) or
/// before the resolver/handler (GraphQL/WS).
///
/// One impl, three transports — override only the `transform_*` method
/// the pipe actually rewrites. A no-op default means "doesn't apply to
/// this transport", not "skip validation".
pub trait GlobalPipe: Layer {
    /// Inspect or rewrite the JSON HTTP body in place. Non-JSON requests
    /// skip the pipe entirely (no parse attempted). Return [`PipeError`]
    /// to reject the request with a `400`. Default no-op.
    fn transform_body(&self, _value: &mut serde_json::Value) -> Result<(), PipeError> {
        Ok(())
    }

    /// Inspect or rewrite the GraphQL operation variables in place. Runs
    /// before any resolver fires. Return [`PipeError`] to reject the
    /// operation. Default no-op.
    fn transform_graphql_variables(
        &self,
        _value: &mut serde_json::Value,
    ) -> Result<(), PipeError> {
        Ok(())
    }

    /// Inspect or rewrite a WebSocket message's `data` payload in place.
    /// Runs before the `#[subscribe_message]` handler. `event` names the
    /// incoming message so a pipe can opt out per event when needed.
    /// Return [`PipeError`] to reject the message. Default no-op.
    fn transform_ws_data(
        &self,
        _event: &str,
        _value: &mut serde_json::Value,
    ) -> Result<(), PipeError> {
        Ok(())
    }
}

impl<T: GlobalPipe + ?Sized> GlobalPipe for Arc<T> {
    fn transform_body(&self, value: &mut serde_json::Value) -> Result<(), PipeError> {
        (**self).transform_body(value)
    }

    fn transform_graphql_variables(
        &self,
        value: &mut serde_json::Value,
    ) -> Result<(), PipeError> {
        (**self).transform_graphql_variables(value)
    }

    fn transform_ws_data(
        &self,
        event: &str,
        value: &mut serde_json::Value,
    ) -> Result<(), PipeError> {
        (**self).transform_ws_data(event, value)
    }
}
