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
//! and attach it via [`HttpEndpointWrap::new`] or
//! [`HttpEndpointWrap::with_priority`].

use nest_rs_core::Container;
use poem::Response;
use poem::endpoint::BoxEndpoint;

type WrapFn = Box<
    dyn Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
        + Send
        + Sync,
>;

/// Canonical priority bands for transport-edge wraps, mirroring the
/// documented HTTP wrap order (outermost → innermost):
///
/// ```text
///   infra #[interceptor]  →  global interceptor pool  →  global filter pool
///   →  DbContext  →  routing (per-route shaper → handler)
/// ```
///
/// The transport iterates `HttpEndpointWrap` entries sorted by priority
/// ascending; lower priority is applied first and therefore ends up
/// innermost. Guards have **no** band here: the guard pool executes inside
/// the per-route shaper (post-routing, so it reads `#[public]`), at the
/// self-mount edge (`SelfMountGuardWrap`), or in-band (GraphQL operation
/// guard). Consequence carried deliberately: a mutating request opens its
/// transaction *before* the guard chain denies it — the denial is a non-2xx
/// response, so the empty transaction rolls back (fail-secure holds; the
/// cost is a wasted `BEGIN`/`ROLLBACK`, to be removed by a lazy executor).
///
/// Insertion order is the tiebreaker, so calls within the same band keep
/// the user's declared order.
pub mod priority {
    /// Innermost band — installs the ambient DB executor around routing.
    /// Sits *inside* the global filter pool so a transport-edge filter
    /// mapping an `Err` can never turn a rollback into a commit.
    pub const DATA_CONTEXT: i32 = -10;
    /// Global filter pool (`use_filters_global`) — maps errors escaping
    /// the routing tree (including 404s and self-mount errors).
    pub const FILTERS: i32 = 50;
    /// Global interceptor pool (`use_interceptors_global`) — wraps the
    /// routing tree: sees every request/response, including guard denials,
    /// 404s and self-mounted surfaces. Sits *inside* infra interceptors so
    /// tracing observes the pool.
    pub const POOL_INTERCEPTORS: i32 = 90;
    /// Outermost band — infra `#[interceptor]` wraps (tracing, timing)
    /// brought by module imports, outside the application pool.
    pub const INTERCEPTORS: i32 = 100;
}

/// Discovery metadata attached at boot. The HTTP transport collects every
/// `HttpEndpointWrap` at `configure` time, sorts by [`priority()`], and
/// folds them around the assembled route (after per-route layers, before
/// CORS / server header).
///
/// The wrap closure receives the container so it can resolve providers it
/// needs (e.g. a global registry of `GuardSpec`s).
///
/// [`priority()`]: HttpEndpointWrap::priority
pub struct HttpEndpointWrap {
    priority: i32,
    wrap: WrapFn,
}

impl HttpEndpointWrap {
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
