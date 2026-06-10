//! GraphQL / WS per-handler chain helpers. Emitted inline at the start
//! of every `#[query]` / `#[mutation]` / `#[field_resolver]` /
//! `#[subscribe_message]` by the matching macro.
//!
//! Each fn composes the global + provider-scope + handler-scope chain,
//! dedups by `TypeId`, and runs the transport-specific check
//! (`check_graphql` / `check_ws_message`).

use std::any::TypeId;

use nest_rs_core::Container;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};
use nest_rs_ws::WsClient;
use serde_json::Value;

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain, dedup_bucket};

use crate::Guard;
use crate::dispatch::denial_convert::denial_to_graphql_error;
use crate::dispatch::scoped_spec::{ScopedGuardSpec, resolve_specs};
use crate::registry::GuardSpecs;

/// GraphQL shaper helper. Called by `#[resolver]` at the start of every
/// resolver method. Dedups per-resolver guards against the global chain.
///
/// GraphQL pipes ([`nest_rs_pipes::GlobalPipe::transform_graphql_variables`])
/// are not invoked here — variables live at the operation level, not per
/// resolver, so wiring them belongs at the GraphQL transport's request
/// entry. The trait method exists today for surface symmetry; runtime
/// wiring at the operation level is queued.
pub async fn run_layered_graphql_chain(
    ctx: &GraphqlContext<'_>,
    container: &Container,
    controller_guards: &[ScopedGuardSpec],
    method_guards: &[ScopedGuardSpec],
    force_guards: &[TypeId],
    route_label: &str,
) -> std::result::Result<(), GraphqlError> {
    let mut global: Vec<ResolvedLayer<dyn Guard>> = Vec::new();
    if let Some(specs) = container.get::<GuardSpecs>() {
        for spec in &specs.0 {
            if let Some(layer) = spec.resolve(container) {
                global.push(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSite::Global,
                    layer,
                });
            }
        }
    }
    let controller = resolve_specs(container, controller_guards, LayerSite::Controller);
    let method = resolve_specs(container, method_guards, LayerSite::Method);
    let chain = compose_chain::<dyn Guard>(
        dedup_bucket(global),
        controller,
        method,
        force_guards,
        route_label,
    );
    for entry in &chain {
        if let Err(denial) = entry.layer.check_graphql(ctx).await {
            return Err(denial_to_graphql_error(denial));
        }
    }
    Ok(())
}

/// WS shaper helper. Called by `#[messages]` at the start of every
/// `#[subscribe_message]` handler. Dedups per-message guards against the
/// global chain.
///
/// WS pipes ([`nest_rs_pipes::GlobalPipe::transform_ws_data`]) are not
/// invoked here — the `#[messages]` macro composes its own per-event
/// chain table inline (so pipe wiring belongs there). The trait method
/// exists today for surface symmetry; wiring at the event-dispatcher
/// level is queued.
#[allow(clippy::too_many_arguments)]
pub async fn run_layered_ws_chain(
    client: &WsClient,
    event: &str,
    data: &Value,
    container: &Container,
    controller_guards: &[ScopedGuardSpec],
    method_guards: &[ScopedGuardSpec],
    force_guards: &[TypeId],
    route_label: &str,
) -> std::result::Result<(), String> {
    let mut global: Vec<ResolvedLayer<dyn Guard>> = Vec::new();
    if let Some(specs) = container.get::<GuardSpecs>() {
        for spec in &specs.0 {
            if let Some(layer) = spec.resolve(container) {
                global.push(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSite::Global,
                    layer,
                });
            }
        }
    }
    let controller = resolve_specs(container, controller_guards, LayerSite::Controller);
    let method = resolve_specs(container, method_guards, LayerSite::Method);
    let chain = compose_chain::<dyn Guard>(
        dedup_bucket(global),
        controller,
        method,
        force_guards,
        route_label,
    );
    for entry in &chain {
        if let Err(denial) = entry.layer.check_ws_message(client, event, data).await {
            return Err(denial.message().to_owned());
        }
    }
    Ok(())
}
