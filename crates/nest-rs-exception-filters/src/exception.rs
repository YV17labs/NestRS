//! [`ExceptionFilter`] — catches a single typed exception across every
//! transport that can carry one.

use std::error::Error as StdError;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};
use nest_rs_ws::WsClient;
use poem::Response;
use serde_json::Value as JsonValue;

/// Catches a typed exception thrown by a handler and maps it to a
/// transport-appropriate result.
///
/// `ExceptionFilter` extends [`Layer`] so it plugs into the same
/// dedup-by-`TypeId` chain as guards, interceptors, pipes, and filters.
/// Each impl declares the concrete error type it claims via
/// [`Self::Exception`]; non-matching errors fall through to the next
/// exception filter in the chain, then to any outer
/// [`Filter`](nest_rs_filters::Filter), then back to the transport's
/// default error renderer.
///
/// The bound on [`Self::Exception`] is what each transport's downcast
/// requires: anything carryable as a `Box<dyn std::error::Error + Send +
/// Sync + 'static>` works. `poem::Error::downcast`, `anyhow::Error::downcast`,
/// and `async_graphql::Error::source().and_then(downcast_ref)` all share
/// this constraint.
///
/// Override only the `catch_*` method(s) the filter targets — `catch`
/// stays required (HTTP is the most common throw site, and a silent
/// default would mask a wiring mistake), the others inherit defaults
/// that pass the error through unchanged so the next layer can try.
#[async_trait]
pub trait ExceptionFilter: Layer {
    /// The concrete exception this filter catches.
    type Exception: StdError + Send + Sync + 'static;

    /// HTTP entry — required. Called with the typed `Exception`
    /// extracted from a `poem::Error` via downcast.
    async fn catch(&self, exception: Self::Exception) -> Response;

    /// GraphQL entry. Called with the typed `Exception` extracted from
    /// an `async_graphql::Error`'s source via downcast; the returned
    /// [`GraphqlError`] replaces the original. Default returns the
    /// exception's `Display` as a plain message — implementors can do
    /// better by overriding.
    ///
    /// Takes `&Self::Exception` (not by value like
    /// [`Self::catch`](Self::catch)) because async-graphql stores the
    /// underlying source as an `Arc<dyn Any + Send + Sync>`, which only
    /// hands out references.
    async fn catch_graphql<'a>(
        &self,
        _ctx: &GraphqlContext<'a>,
        exception: &Self::Exception,
    ) -> GraphqlError {
        GraphqlError::new(exception.to_string())
    }

    /// WS entry. Called with the typed `Exception` extracted from a
    /// message handler's error via downcast; the returned JSON value
    /// becomes the reply payload (typically an error frame). Default
    /// returns a `{"error": "<message>"}` JSON object — override to
    /// produce a richer envelope.
    ///
    /// Takes `&Self::Exception` (not by value like
    /// [`Self::catch`](Self::catch)) because the WS dispatcher uses
    /// `anyhow::Error::downcast_ref` to keep the original error available
    /// for any outer filter that does not match.
    async fn catch_ws(
        &self,
        _client: &WsClient,
        _event: &str,
        exception: &Self::Exception,
    ) -> JsonValue {
        serde_json::json!({ "error": exception.to_string() })
    }
}
