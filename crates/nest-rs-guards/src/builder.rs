//! Extension traits that add the global Layer-System APIs to
//! [`AppBuilder`](nest_rs_core::AppBuilder):
//!
//! - [`AppBuilderGuardsExt::use_guards_global`] — register guards once,
//!   applied to every transport.
//! - [`AppBuilderPipesExt::use_pipes_global`] — register
//!   request-body pipes once, applied to every JSON HTTP handler.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::{AppBuilder, Layer, RequestScope};
use nest_rs_http::HttpInterceptorMeta;
use nest_rs_interceptors::{Interceptor, Next};
use poem::{Request, Response, Result};

use crate::Guard;
use crate::integration::denial_to_http_response;
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
        // Seed the chain for the per-route `LayersRouteInterceptor` (which
        // dedups global+controller+method by `TypeId`) AND for the
        // transport-level [`GlobalGuardsHttpInterceptor`] that wraps every
        // poem endpoint at HTTP configure-time. The latter covers
        // self-mounting endpoints (`/graphql`, MCP, WS upgrade) that don't
        // go through the per-route shaper — without it, a `use_guards_global`
        // registration would silently miss those routes.
        let interceptor: Arc<dyn Interceptor> = Arc::new(GlobalGuardsHttpInterceptor);
        self.provide(GuardSpecs(collected))
            .provide_meta(HttpInterceptorMeta::new(interceptor))
    }
}

/// Adds `.use_pipes_global(...)` to [`AppBuilder`] — the NestJS
/// `useGlobalPipes` analog. Each pipe runs before every JSON HTTP handler;
/// per-route opt-out via `#[no_pipes]`.
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

/// Transport-level interceptor that applies global guards to **every** poem
/// endpoint the HTTP transport assembles — including self-mounting endpoints
/// (`/graphql`, MCP, WS upgrade) that don't traverse the per-route
/// [`LayersRouteInterceptor`](crate::integration::LayersRouteInterceptor)
/// shaper.
///
/// Seeded automatically by [`AppBuilderGuardsExt::use_guards_global`] via an
/// [`HttpInterceptorMeta`]: the HTTP transport reads those metas at configure
/// time and folds the interceptor outermost around the assembled tree. The
/// per-route shaper still runs its own dedup-by-`TypeId` chain (catching
/// controller/method declarations) — so a guard listed both globally and
/// per-route is executed once at the transport seam and skipped by the route
/// shaper.
pub struct GlobalGuardsHttpInterceptor;

impl Layer for GlobalGuardsHttpInterceptor {}

#[async_trait]
impl Interceptor for GlobalGuardsHttpInterceptor {
    async fn intercept(&self, mut req: Request, next: Next<'_>) -> Result<Response> {
        let Some(scope) = req.extensions().get::<Arc<RequestScope>>().cloned() else {
            return next.run(req).await;
        };
        let container = scope.root();
        if let Some(specs) = container.get::<GuardSpecs>() {
            for spec in &specs.0 {
                if let Some(guard) = spec.resolve(container)
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
