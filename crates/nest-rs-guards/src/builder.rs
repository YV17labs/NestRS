//! Extension traits that add the global Layer-System APIs to
//! [`AppBuilder`](nest_rs_core::AppBuilder):
//!
//! - [`AppBuilderGuardsExt::use_guards_global`] — register guards once,
//!   applied to every transport.
//! - [`AppBuilderPipesExt::use_pipes_global`] — register
//!   request-body pipes once, applied to every JSON HTTP handler.

use nest_rs_core::AppBuilder;
use nest_rs_core::layer_chain::ResolvedLayer;
use nest_rs_http::{GlobalGuardsActive, HttpBootCheck, SelfMountGuardWrap};
use nest_rs_interceptors::InterceptorExt;
use poem::EndpointExt;

use crate::Guard;
#[cfg(feature = "graphql")]
use crate::dispatch::GlobalPoolOperationGuard;
use crate::dispatch::denial_to_http_response;
use crate::registry::{GuardSpec, GuardSpecs, PipeSpec, PipeSpecs};
#[cfg(feature = "graphql")]
use nest_rs_graphql::{FallbackOperationGuard, GraphqlVariablePipe};
use nest_rs_ws::WsDataPipe;

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
        // Seed `GuardSpecs` — read by the per-route `RouteShaper`, which runs
        // the global guard pool (deduped against controller / method
        // declarations) *after* routing so a guard sees `#[public]`. Plus the
        // two single-site executors for surfaces without a shaper:
        //
        // - `SelfMountGuardWrap` — a `Guarded` self-mount (WS upgrade) gets
        //   the global chain at its HTTP edge;
        // - `FallbackOperationGuard` — `/graphql` is `Exempt` at the edge and
        //   gates per operation; when the app registers no
        //   `dyn GraphqlOperationGuard` bridge, the global pool runs there
        //   in-band, so a forgotten bridge module never leaves operations
        //   unguarded. A registered bridge replaces the fallback (it runs
        //   the same guards itself — nothing runs twice).
        let active = !collected.is_empty();
        let mut builder = self.provide(GuardSpecs(collected));
        #[cfg(feature = "graphql")]
        {
            builder = builder.provide(FallbackOperationGuard(GlobalPoolOperationGuard::factory));
        }
        let builder = builder
            .provide_meta(SelfMountGuardWrap::new(|container, endpoint| {
                let chain = container
                    .get::<GuardSpecs>()
                    .map(|specs| specs.resolve_chain(container, "self-mount edge"))
                    .unwrap_or_default();
                InterceptorExt::interceptor(endpoint, GuardsHttpFold { chain })
                    .map_to_response()
                    .boxed()
            }))
            // A global guard whose provider was never registered would
            // resolve to `None` and silently drop — every route would lose
            // its fail-secure net. Fail boot instead, naming the guards.
            .provide_meta(HttpBootCheck::new(|container| {
                let Some(specs) = container.get::<GuardSpecs>() else {
                    return Ok(());
                };
                let missing: Vec<&str> = specs
                    .0
                    .iter()
                    .filter(|s| s.resolve(container).is_none())
                    .map(|s| s.name)
                    .collect();
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "global guard(s) not resolvable from the container: {} — import the \
                         module that provides them; an unresolvable global guard would \
                         silently drop and leave every route unguarded",
                        missing.join(", "),
                    ))
                }
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
        // Bridge the global pipes onto `/graphql` operation variables — the
        // operation-level analog of the HTTP `transform_body` site. The endpoint
        // (`nest-rs-graphql`) owns the call; this crate owns `PipeSpecs`, so it
        // seeds the fn (same seeded-fn-pointer pattern as `FallbackOperationGuard`).
        #[cfg(feature = "graphql")]
        let builder = builder.provide(GraphqlVariablePipe(run_graphql_variable_pipes));
        // WS per-message data pipes (`transform_ws_data`). The gateway resolves
        // this bridge at mount (it has the container) and folds it over each
        // message's `data` after guards, before dispatch.
        let builder = builder.provide(WsDataPipe(run_ws_data_pipes));
        builder.provide_meta(HttpBootCheck::new(|container| {
            let Some(specs) = container.get::<PipeSpecs>() else {
                return Ok(());
            };
            let missing: Vec<&str> = specs
                .0
                .iter()
                .filter(|s| s.resolve(container).is_none())
                .map(|s| s.name)
                .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "global pipe(s) not resolvable from the container: {} — import the \
                     module that provides them; an unresolvable global pipe would \
                     silently drop its edge validation",
                    missing.join(", "),
                ))
            }
        }))
    }
}

/// The seed behind [`GraphqlVariablePipe`]: fold every registered global pipe's
/// `transform_graphql_variables` over an operation's variables. Lives here (not
/// in `nest-rs-graphql`) because it reads the `PipeSpecs` registry this crate
/// owns; the endpoint only holds the fn pointer.
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
/// `transform_ws_data` over a message's `data`. Lives here (not in `nest-rs-ws`)
/// because it reads the `PipeSpecs` registry this crate owns.
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

/// Internal adapter — runs the composed global guard chain inside an
/// `Interceptor`-shaped wrap. Used by `SelfMountGuardWrap` to apply the global
/// guard chain at a `Guarded` self-mounted endpoint's edge (it has no
/// per-route shaper). The chain is resolved eagerly at configure time — the
/// container is final there, so a broken chain surfaces at boot, not on the
/// first request.
struct GuardsHttpFold {
    chain: Vec<ResolvedLayer<dyn Guard>>,
}

impl nest_rs_core::Layer for GuardsHttpFold {}

#[async_trait::async_trait]
impl nest_rs_interceptors::Interceptor for GuardsHttpFold {
    async fn intercept(
        &self,
        mut req: poem::Request,
        next: nest_rs_interceptors::Next<'_>,
    ) -> poem::Result<poem::Response> {
        for entry in &self.chain {
            if let Err(denial) = entry.layer.check_http(&mut req).await {
                return Ok(denial_to_http_response(denial));
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
                guard = %s.name,
                hint = "authn should precede authz",
                "guard order looks reversed",
            );
        }
        if is_authz {
            saw_authz = true;
        }
    }
}

#[cfg(test)]
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

    // The full WS bridge: `use_pipes_global` seeds `WsDataPipe(run_ws_data_pipes)`,
    // the gateway resolves it into a container-bound fold, and the fold runs the
    // registered global pipe's `transform_ws_data`. Only the (trivial) call from
    // `handle_text` is not covered here.
    #[test]
    fn ws_data_pipe_bridge_folds_transform_ws_data() {
        let container = Container::builder()
            .provide(WsUpcase)
            .provide(PipeSpecs(vec![pipe::<WsUpcase>()]))
            .provide(nest_rs_ws::WsDataPipe(run_ws_data_pipes))
            .build();

        let fold = nest_rs_ws::resolve_ws_data_pipe(&container).expect("a bridge is registered");

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
        assert!(nest_rs_ws::resolve_ws_data_pipe(&container).is_none());
    }
}
