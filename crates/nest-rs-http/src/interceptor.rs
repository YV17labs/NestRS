//! Discovery metadata attached at boot to wrap the assembled HTTP endpoint
//! transport-wide.
//!
//! Carries a closure, not an `Interceptor`: `nest-rs-interceptors` depends on
//! this crate through `nest-rs-graphql`, so naming the trait here is a cycle.

use nest_rs_core::Container;
use poem::Response;
use poem::endpoint::BoxEndpoint;

type WrapFn = Box<
    dyn Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
        + Send
        + Sync,
>;

/// Canonical priority bands for transport-edge wraps (outermost → innermost):
///
/// ```text
///   infra #[interceptor]  →  global interceptor pool  →  global filter pool
///   →  DbContext  →  routing (per-route shaper → handler)
/// ```
///
/// Lower applies first and ends innermost; insertion order breaks a tie. Guards
/// have no band: they run after routing, so `DbContext` wraps them — its
/// `BEGIN` is lazy, so a denied request opens no transaction.
pub mod priority {
    /// Innermost band — installs the ambient DB executor around routing.
    /// Inside the filter pool, so a filter mapping an `Err` cannot turn a
    /// rollback into a commit.
    pub const DATA_CONTEXT: i32 = -10;
    /// Global filter pool (`use_filters_global`) — maps errors escaping
    /// the routing tree (including 404s and self-mount errors).
    pub const FILTERS: i32 = 50;
    /// Where the transport renders a still-unhandled `Err` into its
    /// `Response`, so every band above sees a 404 or 405 as a response. Not a
    /// wrap an app can register.
    pub const ERROR_RESOLVE: i32 = 70;
    /// Global interceptor pool (`use_interceptors_global`) — wraps the
    /// routing tree: sees every request/response, including guard denials,
    /// 404s and self-mounted surfaces.
    pub const POOL_INTERCEPTORS: i32 = 90;
    /// Outermost band — infra `#[interceptor]` wraps (tracing, timing)
    /// brought by module imports, outside the application pool.
    pub const INTERCEPTORS: i32 = 100;
}

/// Discovery metadata the HTTP transport folds, sorted by
/// [`priority()`](HttpEndpointWrap::priority), around the assembled route.
pub struct HttpEndpointWrap {
    priority: i32,
    wrap: WrapFn,
}

impl HttpEndpointWrap {
    /// Construct from any wrap closure with the default priority
    /// ([`priority::INTERCEPTORS`]).
    pub fn new<F>(wrap: F) -> Self
    where
        F: Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
            + Send
            + Sync
            + 'static,
    {
        Self::with_priority(priority::INTERCEPTORS, wrap)
    }

    /// Construct with an explicit priority band; lower ends up innermost.
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

    /// Priority band; see [`priority`] for the framework's bands.
    pub fn priority(&self) -> i32 {
        self.priority
    }

    /// Apply the wrap to `endpoint`.
    pub fn wrap(
        &self,
        container: &Container,
        endpoint: BoxEndpoint<'static, Response>,
    ) -> BoxEndpoint<'static, Response> {
        (self.wrap)(container, endpoint)
    }
}
