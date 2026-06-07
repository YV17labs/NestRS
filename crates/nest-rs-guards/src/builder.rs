//! Extension traits that add the global Layer-System APIs to
//! [`AppBuilder`](nest_rs_core::AppBuilder):
//!
//! - [`AppBuilderGuardsExt::use_guards_global`] — register guards once,
//!   applied to every transport.
//! - [`AppBuilderPipesExt::use_pipes_global`] — register
//!   request-body pipes once, applied to every JSON HTTP handler.

use std::sync::Arc;

use nest_rs_core::{AppBuilder, RequestScope};
use nest_rs_http::{HttpEndpointWrap, endpoint_wrap_priority};
use nest_rs_interceptors::InterceptorExt;
use poem::EndpointExt;

use crate::dispatch::denial_to_http_response;
use crate::registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs};

/// Adds `.use_guards_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs_guards::{AppBuilderGuardsExt, guard};
///
/// App::builder()
///     .use_guards_global([guard::<AuthGuard>(), guard::<AuthzGuard>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
///
/// Declaration order matters — the runtime chain runs in the order you list
/// the guards (with [`Layer::priority`](nest_rs_core::Layer::priority) as an
/// optional tiebreaker). If you list `AuthzGuard` before `AuthGuard` you'll
/// get an authorization check before authentication has attached the
/// principal — usually a bug.
pub trait AppBuilderGuardsExt: Sized {
    fn use_guards_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>;
}

impl AppBuilderGuardsExt for AppBuilder {
    fn use_guards_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>,
    {
        let collected: Vec<GuardSpec> = specs.into_iter().collect();
        validate_order_by_name(&collected);
        // Seed `GuardSpecs` (read by the per-route `RouteShaper` for
        // TypeId dedup against controller / method declarations) AND an
        // `HttpEndpointWrap` at the GUARDS priority band so the HTTP
        // transport runs every global guard's `check_http` once around the
        // assembled endpoint — covers self-mounting routes (`/graphql`,
        // MCP, WS upgrade) that don't traverse the per-route shaper. The
        // wrap is a closure (not a named `Interceptor` struct) for
        // symmetry with `use_interceptors_global` / `use_filters_global`.
        self.provide(GuardSpecs(collected))
            .provide_meta(HttpEndpointWrap::with_priority(
                endpoint_wrap_priority::GUARDS,
                |container, endpoint| {
                    let endpoint = endpoint.map_to_response().boxed();
                    InterceptorExt::interceptor(
                        endpoint,
                        GuardsHttpFold {
                            container: container.clone(),
                        },
                    )
                    .map_to_response()
                    .boxed()
                },
            ))
    }
}

/// Adds `.use_pipes_global(...)` to [`AppBuilder`]. Each pipe runs before
/// every JSON HTTP handler; per-route opt-out via `#[no_pipes]`.
pub trait AppBuilderPipesExt: Sized {
    fn use_pipes_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>;
}

impl AppBuilderPipesExt for AppBuilder {
    fn use_pipes_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>,
    {
        self.provide(PipeSpecs(specs.into_iter().collect()))
    }
}

/// Internal adapter — runs the global `GuardSpecs` chain inside an
/// `Interceptor`-shaped wrap. The HTTP transport only knows how to fold
/// `HttpEndpointWrap` closures around the assembled endpoint, and those
/// closures express their work as poem endpoint wrapping; `Interceptor`
/// is poem's `(req, next) -> response` shape. This type bridges the two
/// — every other Layer-System global builder follows the same pattern
/// via an inline closure (interceptors, filters), but guards need
/// `RequestScope` from `req.extensions()` so a typed struct keeps the
/// resolution clean.
struct GuardsHttpFold {
    container: nest_rs_core::Container,
}

impl nest_rs_core::Layer for GuardsHttpFold {}

#[async_trait::async_trait]
impl nest_rs_interceptors::Interceptor for GuardsHttpFold {
    async fn intercept(
        &self,
        mut req: poem::Request,
        next: nest_rs_interceptors::Next<'_>,
    ) -> poem::Result<poem::Response> {
        // Prefer the request-scoped container (carries per-request
        // overrides) when `RequestScopeEndpoint` installed one; fall
        // back to the captured root container so guards still run on
        // endpoints that bypass the request-scope wrapper.
        let container = req
            .extensions()
            .get::<Arc<RequestScope>>()
            .map(|scope| scope.root().clone())
            .unwrap_or_else(|| self.container.clone());
        if let Some(specs) = container.get::<GuardSpecs>() {
            for spec in &specs.0 {
                if let Some(guard) = spec.resolve(&container)
                    && let Err(denial) = guard.check_http(&mut req).await
                {
                    return Ok(denial_to_http_response(denial));
                }
            }
        }
        next.run(req).await
    }
}

/// Log a warning if `Authorization`-sounding precedes `Auth`-sounding in
/// the declaration list. Best-effort static name heuristic; ordering at
/// runtime is whatever the dev listed (no auto-reorder).
fn validate_order_by_name(specs: &[GuardSpec]) {
    let mut saw_authz = false;
    for s in specs {
        let name = s.name.to_ascii_lowercase();
        let is_authz = name.contains("authz") || name.contains("ability");
        let is_authn = (name.contains("auth") && !is_authz) || name.contains("authn");
        if saw_authz && is_authn {
            tracing::warn!(
                target: "nest_rs::layers",
                "global guard order looks reversed — `{}` (looks like authn) follows a guard that looks like authz; authn should precede authz",
                s.name,
            );
        }
        if is_authz {
            saw_authz = true;
        }
    }
}
