//! Extension traits that add the global Layer-System APIs to
//! [`AppBuilder`]:
//!
//! - [`AppBuilderGuardsExt::use_guards_global`] — register guards once,
//!   applied to every transport.
//! - [`AppBuilderPipesExt::use_pipes_global`] — register
//!   request-body pipes once, applied to every JSON HTTP handler.

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
use crate::registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs};
#[cfg(feature = "graphql")]
use nest_rs_graphql::__private::{FallbackOperationGuard, FederationGate, GraphqlVariablePipe};
#[cfg(feature = "mcp")]
use nest_rs_mcp::__private::FallbackMcpGuard;
#[cfg(feature = "ws")]
use nest_rs_ws::__private::WsDataPipe;

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
        // `SelfMountGuardWrap` at a `Guarded` self-mount's edge, and a fallback on
        // `/graphql` and `/mcp` when no bridge is registered, so a forgotten
        // bridge never leaves operations unguarded.
        let active = !collected.is_empty();
        #[cfg_attr(
            not(any(feature = "graphql", feature = "mcp")),
            expect(
                unused_mut,
                reason = "only the graphql / mcp fallback op-guards reassign `builder`"
            )
        )]
        let mut builder = self.provide(GuardSpecs(collected));
        #[cfg(feature = "graphql")]
        {
            builder = builder.provide(FallbackOperationGuard(GlobalPoolOperationGuard::factory));
            // `_service` / `_entities` resolve above the merged root, out of the
            // resolver-site chain's reach. Seeded even for an empty pool.
            builder = builder.provide(FederationGate(GlobalPoolFederationGuard::factory));
        }
        // MCP's no-guard default is deny-all, not pass-through, so the fallback
        // is seeded only for a non-empty pool: it may widen `/mcp` to exactly
        // what the app opted into, never to "open".
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

/// Adds `.use_pipes_global(...)` to [`AppBuilder`]. Each pipe runs before
/// every JSON HTTP handler; per-route opt-out via `#[no_pipes]`.
pub trait AppBuilderPipesExt: Sized {
    /// Register `specs` as the global pipe pool — run before every JSON HTTP
    /// handler unless a route opts out with `#[no_pipes]`.
    fn use_pipes_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>;
}

impl AppBuilderPipesExt for AppBuilder {
    fn use_pipes_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>,
    {
        let builder = self.provide(PipeSpecs(specs.into_iter().collect()));
        // Global pipes over `/graphql` operation variables; the endpoint holds
        // only the fn pointer.
        #[cfg(feature = "graphql")]
        let builder = builder.provide(GraphqlVariablePipe(run_graphql_variable_pipes));
        // WS per-message data pipes, folded over `data` after guards, before dispatch.
        #[cfg(feature = "ws")]
        let builder = builder.provide(WsDataPipe(run_ws_data_pipes));
        builder.provide_meta(HttpBootCheck::new(|container| {
            check_specs_resolvable::<PipeSpecs>(
                container,
                "pipe",
                "an unresolvable global pipe would silently drop its edge validation",
            )
        }))
    }
}

/// The seed behind [`GraphqlVariablePipe`]: fold every registered global pipe's
/// `transform_graphql_variables` over an operation's variables.
#[cfg(feature = "graphql")]
fn run_graphql_variable_pipes(
    container: &nest_rs_core::Container,
    value: &mut serde_json::Value,
) -> std::result::Result<(), nest_rs_pipes::PipeError> {
    if let Some(specs) = container.get::<PipeSpecs>() {
        for spec in &specs.0 {
            if let Some(pipe) = spec.resolve(container) {
                pipe.transform_graphql_variables(value)?;
            }
        }
    }
    Ok(())
}

/// The seed behind [`WsDataPipe`]: fold every registered global pipe's
/// `transform_ws_data` over a message's `data`.
#[cfg(feature = "ws")]
fn run_ws_data_pipes(
    container: &nest_rs_core::Container,
    event: &str,
    value: &mut serde_json::Value,
) -> std::result::Result<(), nest_rs_pipes::PipeError> {
    if let Some(specs) = container.get::<PipeSpecs>() {
        for spec in &specs.0 {
            if let Some(pipe) = spec.resolve(container) {
                pipe.transform_ws_data(event, value)?;
            }
        }
    }
    Ok(())
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

#[cfg(all(test, feature = "ws"))]
mod tests {
    use super::*;
    use nest_rs_core::{Container, Layer};
    use nest_rs_pipes::{GlobalPipe, PipeError};
    use serde_json::{Value, json};

    use crate::registry::pipe;

    /// Uppercases `data.msg`, and rejects the `"boom"` event — exercises both the
    /// transform and the error path of the WS data-pipe bridge.
    #[derive(Default)]
    struct WsUpcase;

    impl Layer for WsUpcase {}

    impl GlobalPipe for WsUpcase {
        fn transform_ws_data(&self, event: &str, value: &mut Value) -> Result<(), PipeError> {
            if event == "boom" {
                return Err(PipeError::new("no boom allowed"));
            }
            if let Some(msg) = value.get("msg").and_then(Value::as_str) {
                let upper = msg.to_uppercase();
                value["msg"] = Value::String(upper);
            }
            Ok(())
        }
    }

    // The full WS bridge, short of the trivial call from `handle_text`.
    #[test]
    fn ws_data_pipe_bridge_folds_transform_ws_data() {
        let container = Container::builder()
            .provide(WsUpcase)
            .provide(PipeSpecs(vec![pipe::<WsUpcase>()]))
            .provide(nest_rs_ws::__private::WsDataPipe(run_ws_data_pipes))
            .build();

        let fold = nest_rs_ws::__private::resolve_ws_data_pipe(&container)
            .expect("a bridge is registered");

        let mut data = json!({ "msg": "hi" });
        fold("chat", &mut data).expect("the transform runs");
        assert_eq!(data["msg"], "HI", "the pipe uppercased the message");

        let mut data = json!({ "msg": "x" });
        let Err(err) = fold("boom", &mut data) else {
            panic!("the `boom` event must be rejected");
        };
        assert_eq!(err.message(), "no boom allowed");
    }

    #[test]
    fn no_bridge_means_no_fold() {
        let container = Container::builder().build();
        assert!(nest_rs_ws::__private::resolve_ws_data_pipe(&container).is_none());
    }
}
