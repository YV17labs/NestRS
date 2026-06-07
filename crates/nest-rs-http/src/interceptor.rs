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
//! and attach it via [`HttpInterceptorMeta::new`] or
//! [`HttpInterceptorMeta::with_priority`].

use nest_rs_core::Container;
use poem::Response;
use poem::endpoint::BoxEndpoint;

type WrapFn = Box<
    dyn Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
        + Send
        + Sync,
>;

/// Canonical priority bands for Layer-System globals, mirroring the
/// documented HTTP wrap order (outermost → innermost):
///
/// ```text
///   Interceptors  →  Filters  →  Guards  →  per-route shaper  →  handler
/// ```
///
/// The transport iterates `HttpInterceptorMeta` entries sorted by priority
/// ascending; lower priority is applied first and therefore ends up
/// innermost. `Interceptors` get the highest priority so their wrap
/// installs *outermost* — that's how an ambient `DbContext` interceptor
/// can install the SeaORM executor before guards (e.g. an `AbilityGuard`
/// that reads it) run.
///
/// Insertion order is the tiebreaker, so calls within the same band keep
/// the user's declared order.
pub mod priority {
    /// Innermost band — runs closest to the handler. Guards reject
    /// before any work happens; they want to sit just outside the
    /// per-route shaper.
    pub const GUARDS: i32 = 0;
    /// Middle band — filters map errors bubbling up from the inner
    /// chain, so they sit outside guards and inside interceptors.
    pub const FILTERS: i32 = 50;
    /// Outermost band — interceptors install ambient state (transaction
    /// scope, request tracing) every other layer needs to observe.
    pub const INTERCEPTORS: i32 = 100;
}

/// Discovery metadata attached at boot. The HTTP transport collects every
/// `HttpInterceptorMeta` at `configure` time, sorts by [`priority()`], and
/// folds them around the assembled route (after per-route layers, before
/// CORS / server header).
///
/// The wrap closure receives the container so it can resolve providers it
/// needs (e.g. a global registry of `GuardSpec`s).
///
/// [`priority()`]: HttpInterceptorMeta::priority
pub struct HttpInterceptorMeta {
    priority: i32,
    wrap: WrapFn,
}

impl HttpInterceptorMeta {
    /// Construct from any wrap closure with the default priority
    /// ([`priority::INTERCEPTORS`]). Use [`Self::with_priority`] when you
    /// need an explicit band — Layer-System globals (guards, filters)
    /// always do so the documented ordering is enforced regardless of
    /// AppBuilder call order.
    pub fn new<F>(wrap: F) -> Self
    where
        F: Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
            + Send
            + Sync
            + 'static,
    {
        Self::with_priority(priority::INTERCEPTORS, wrap)
    }

    /// Construct with an explicit priority band. Lower priority is
    /// applied first by the transport and therefore ends up innermost in
    /// the final endpoint composition.
    pub fn with_priority<F>(priority: i32, wrap: F) -> Self
    where
        F: Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
            + Send
            + Sync
            + 'static,
    {
        Self {
            priority,
            wrap: Box::new(wrap),
        }
    }

    /// Priority band — lower applies first (innermost wrap), higher
    /// applies last (outermost wrap). See [`priority`] for the canonical
    /// bands the framework uses.
    pub fn priority(&self) -> i32 {
        self.priority
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
