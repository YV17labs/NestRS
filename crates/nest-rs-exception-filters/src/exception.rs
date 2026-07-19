//! [`ExceptionFilter`] — catches a single typed exception across every
//! transport that can carry one.

use std::error::Error as StdError;

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::Response;

/// Catches a typed exception thrown by a handler and maps it to a
/// transport-appropriate result.
///
/// `ExceptionFilter` extends [`Layer`] so it plugs into the same
/// dedup-by-`TypeId` chain as guards, interceptors, pipes, and filters.
/// Each impl declares the concrete error type it claims via
/// [`Self::Exception`]; non-matching errors fall through to the next
/// exception filter in the chain, then to any outer
/// `Filter` (`nest_rs_filters::Filter`), then back to the transport's
/// default error renderer.
///
/// The bound on [`Self::Exception`] is what the transport's downcast
/// requires: anything carryable as a `Box<dyn std::error::Error + Send +
/// Sync + 'static>` works (`poem::Error::downcast` shares this constraint).
///
/// HTTP is the only entry — former GraphQL / WS reserved seams were removed
/// until they are actually wired.
#[async_trait]
pub trait ExceptionFilter: Layer {
    /// The concrete exception this filter catches.
    type Exception: StdError + Send + Sync + 'static;

    /// HTTP entry — required. Called with the typed `Exception`
    /// extracted from a `poem::Error` via downcast.
    async fn catch(&self, exception: Self::Exception) -> Response;
}
