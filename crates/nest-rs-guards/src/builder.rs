//! [`AppBuilderGuardsExt::use_guards_global`] — register guards once, applied
//! to every transport.

use nest_rs_core::__private::{ResolvedLayer, check_specs_resolvable};
use nest_rs_core::AppBuilder;
use nest_rs_http::__private::SelfMountGuardWrap;
use nest_rs_http::{GlobalGuardsActive, HttpBootCheck};
use poem::EndpointExt;
use poem::endpoint::BoxEndpoint;

use crate::Guard;
#[cfg(feature = "mcp")]
use crate::dispatch::GlobalPoolMcpGuard;
use crate::dispatch::deny_http;
#[cfg(feature = "graphql")]
use crate::dispatch::{GlobalPoolFederationGuard, GlobalPoolOperationGuard};
use crate::registry::{GuardSpec, GuardSpecs};
#[cfg(feature = "graphql")]
use nest_rs_graphql::__private::{FallbackOperationGuard, FederationGate};
#[cfg(feature = "mcp")]
use nest_rs_mcp::__private::FallbackMcpGuard;

/// Adds `.use_guards_global(...)` to [`AppBuilder`].
///
/// The example on [`guard`](fn@crate::guard) registers through it.
///
/// Declaration order is the runtime order, with
/// [`Layer::priority`](nest_rs_core::Layer::priority) as an optional tiebreaker:
/// list `AuthnGuard` before `AuthzGuard`.
pub trait AppBuilderGuardsExt: Sized {
    /// Register `specs` as the global guard chain, run in list order at the
    /// route shaper — order matters (authn before authz).
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
        // `GuardSpecs` feeds each route's `RouteShaper`, after routing so a guard
        // sees `#[public]`. Surfaces without a shaper get a single-site executor:
        // `SelfMountGuardWrap` at a `Guarded` self-mount's edge, and the pool's
        // runner on `/graphql` and `/mcp`, which a registered bridge joins rather
        // than replaces.
        let active = !collected.is_empty();
        #[cfg_attr(
            not(any(feature = "graphql", feature = "mcp")),
            expect(
                unused_mut,
                reason = "only the graphql / mcp pool runners reassign `builder`"
            )
        )]
        let mut builder = self.provide(GuardSpecs(collected));
        // Each runner is seeded only for a non-empty pool, so an endpoint without
        // one says its own posture: `/graphql` unguarded at `warn`, `/mcp`
        // deny-all — or the registered bridge alone.
        #[cfg(feature = "graphql")]
        {
            if active {
                builder =
                    builder.provide(FallbackOperationGuard(GlobalPoolOperationGuard::factory));
            }
            // `_service` / `_entities` resolve above the merged root, out of the
            // resolver-site chain's reach. Seeded even for an empty pool.
            builder = builder.provide(FederationGate(GlobalPoolFederationGuard::factory));
        }
        #[cfg(feature = "mcp")]
        if active {
            builder = builder.provide(FallbackMcpGuard(GlobalPoolMcpGuard::factory));
        }
        let builder = builder
            .provide_meta(SelfMountGuardWrap::new(|container, endpoint| {
                let chain = container
                    .get::<GuardSpecs>()
                    .map(|specs| specs.resolve_chain(container, "self-mount edge"))
                    .unwrap_or_default();
                if chain.is_empty() {
                    return endpoint;
                }
                SelfMountGuarded {
                    chain,
                    inner: endpoint,
                }
                .boxed()
            }))
            // A global guard whose provider was never registered would
            // resolve to `None` and silently drop — every route would lose
            // its fail-secure net. Fail boot instead, naming the guards.
            .provide_meta(HttpBootCheck::new(|container| {
                check_specs_resolvable::<GuardSpecs>(
                    container,
                    "guard",
                    "an unresolvable global guard would silently drop and leave every route \
                     unguarded",
                )?;
                // Declared-phase ordering + produced/expected principal
                // cross-check on the global chain — a reversed or mismatched
                // pairing fails boot here instead of answering 500 per request.
                crate::dispatch::boot_validate_guards(container, &[], "the global guard chain")
            }));
        if active {
            builder.provide(GlobalGuardsActive)
        } else {
            builder
        }
    }
}

/// Runs the composed global guard chain at a `Guarded` self-mounted endpoint's
/// edge, applied through `SelfMountGuardWrap`. Resolved at configure time, so a
/// broken chain surfaces at boot.
struct SelfMountGuarded {
    chain: Vec<ResolvedLayer<dyn Guard>>,
    inner: BoxEndpoint<'static, poem::Response>,
}

impl poem::Endpoint for SelfMountGuarded {
    type Output = poem::Response;

    async fn call(&self, mut req: poem::Request) -> poem::Result<poem::Response> {
        for entry in &self.chain {
            // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
            // blanket would nest a second boxed future per check.
            if let Err(denial) = entry.layer.as_ref().check_http(&mut req).await {
                return Ok(deny_http(entry.name, denial));
            }
        }
        self.inner.call(req).await
    }
}
