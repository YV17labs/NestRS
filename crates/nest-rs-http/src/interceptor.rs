//! Discovery metadata attached at boot to wrap the assembled HTTP endpoint
//! transport-wide.
//!
//! Carries a wrap closure (not an `Interceptor` instance) so this crate
//! stays free of the `Interceptor` trait — that trait spans transports
//! (HTTP + GraphQL + WS) and lives in `nest-rs-interceptors`, which itself
//! depends on `nest-rs-graphql` and `nest-rs-ws`. Pulling it back here
//! would close the dependency cycle:
//!
//! ```text
//! nest-rs-interceptors → nest-rs-graphql → nest-rs-http → nest-rs-interceptors
//! ```
//!
//! Layer System wraps that need to participate (global guards, global
//! interceptors, global filters, …) construct the wrap closure themselves
//! and attach it via [`HttpInterceptorMeta::new`].

use nest_rs_core::Container;
use poem::Response;
use poem::endpoint::BoxEndpoint;

type WrapFn = Box<
    dyn Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
        + Send
        + Sync,
>;

/// Discovery metadata attached at boot. The HTTP transport collects every
/// `HttpInterceptorMeta` at `configure` time and folds them around the
/// assembled route (after per-route layers, before CORS / server header).
///
/// The wrap closure receives the container so it can resolve providers it
/// needs (e.g. a global registry of `GuardSpec`s).
pub struct HttpInterceptorMeta {
    wrap: WrapFn,
}

impl HttpInterceptorMeta {
    /// Construct from any wrap closure. The closure takes the live
    /// container and the partially-assembled endpoint, and returns the
    /// wrapped endpoint.
    pub fn new<F>(wrap: F) -> Self
    where
        F: Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
            + Send
            + Sync
            + 'static,
    {
        Self {
            wrap: Box::new(wrap),
        }
    }

    /// Apply the wrap to `endpoint`. Called once per meta at
    /// `HttpTransport::configure` time.
    pub fn wrap(
        &self,
        container: &Container,
        endpoint: BoxEndpoint<'static, Response>,
    ) -> BoxEndpoint<'static, Response> {
        (self.wrap)(container, endpoint)
    }
}
