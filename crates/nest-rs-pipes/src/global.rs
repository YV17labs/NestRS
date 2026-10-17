//! Global pipes — input shaping registered once for the whole app.
//!
//! A [`GlobalPipe`] inspects or rewrites the raw input **before** it is decoded
//! for the handler, handed it with the [`ArgumentKind`] that says which input it
//! is: an HTTP request's JSON body, a `/graphql` operation's variables (on a
//! POST and on the graphql-ws socket alike), a WebSocket message's `data`.
//! Registered once with `App::builder().use_pipes_global(...)`, it applies to
//! each of those inputs in the app — no per-route opt-in. The framework gives
//! pipes the [`LayerKind::Pipe`](nest_rs_core::LayerKind) execution slot: they
//! run after Guards and before the handler. An HTTP route opts out with
//! `#[no_pipes]`. MCP tool arguments and queue payloads run no global pipe:
//! bind a per-argument pipe there.
//!
//! ## Defining a global pipe
//!
//! ```
//! use nest_rs_core::{Layer, UnitContext, injectable};
//! use nest_rs_pipes::{ArgumentKind, GlobalPipe, PipeError};
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct StripUnknownFields;
//!
//! impl Layer for StripUnknownFields {}
//!
//! impl GlobalPipe for StripUnknownFields {
//!     fn transform(
//!         &self,
//!         _cx: &UnitContext<'_>,
//!         argument: ArgumentKind,
//!         input: &mut serde_json::Value,
//!     ) -> Result<(), PipeError> {
//!         if argument == ArgumentKind::Body {
//!             // …
//!         }
//!         Ok(())
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! ## Registering globally
//!
//! `App::builder().use_pipes_global([pipe::<StripUnknownFields>()])`, through
//! [`AppBuilderPipesExt`](crate::AppBuilderPipesExt); the example on
//! [`pipe`](fn@crate::pipe) runs it.

use std::sync::Arc;

use nest_rs_core::{Layer, UnitContext};

use crate::PipeError;

/// A request-input validator/transformer applied to every JSON HTTP body,
/// `/graphql` operation's variables and WebSocket message's `data`. Runs after
/// `Guard`s, before the extractor (HTTP) or before the resolver/handler
/// (GraphQL/WS).
///
/// One entry for every edge: `argument` says which input it is handed, and
/// `cx` the unit it belongs to — its edge, its handler's `Reflector`, the
/// edge's view of it. The default leaves every input unchanged.
pub trait GlobalPipe: Layer {
    /// Inspect or rewrite one unit's raw JSON `input` in place, before it is
    /// decoded. Return [`PipeError`] to refuse the unit: the edge answers it as
    /// its own validation error (a `400` on HTTP, a GraphQL error, an error
    /// frame).
    ///
    /// An HTTP body that is not JSON reaches no pipe.
    fn transform(
        &self,
        cx: &UnitContext<'_>,
        argument: ArgumentKind,
        input: &mut serde_json::Value,
    ) -> Result<(), PipeError> {
        let _ = (cx, argument, input);
        Ok(())
    }
}

impl<T: GlobalPipe + ?Sized> GlobalPipe for Arc<T> {
    fn transform(
        &self,
        cx: &UnitContext<'_>,
        argument: ArgumentKind,
        input: &mut serde_json::Value,
    ) -> Result<(), PipeError> {
        (**self).transform(cx, argument, input)
    }
}

/// Which raw input a [`GlobalPipe`] is handed. A unit's name cannot say it:
/// the `/graphql` request is an `http.request` like a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArgumentKind {
    /// An HTTP request's JSON body.
    Body,
    /// A GraphQL operation's variables.
    Variables,
    /// A WebSocket message's `data`.
    Data,
}
