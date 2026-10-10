//! WS gateway chains, composed once at mount: the upgrade's own guards and each
//! `#[subscribe_message]` event's per-message table, both through the
//! `compose_chain` dedup every other site uses.
//!
//! `#[messages]`' boot check refuses a declaration no imported module provides
//! before the gateway mounts; the mount itself, which has no `Result`, files
//! the same refusal and denies, so it never runs a chain without a layer.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::__private::{compose_chain, dedup_bucket, refuse_site};
use nest_rs_core::Container;
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_ws::{WsClient, WsError, WsMessageCheck};
use poem::endpoint::BoxEndpoint;
use poem::{EndpointExt, Response};

use crate::dispatch::denial_convert::denial_to_ws_error;
use crate::dispatch::route_shaper::refused_route;
use crate::dispatch::scoped_spec::{
    ScopedGuardSpec, report_unresolved, resolve_global_guards, resolve_scoped,
};
use crate::dispatch::validate::boot_validate_guards;
use crate::{Denial, Guard, GuardAsWsMessageCheck, GuardExt};

/// What an event's chain is named in boot logs and refusals.
fn event_site(event: &str) -> String {
    format!("ws {event}")
}

/// What a gateway's upgrade chain is named in boot logs and refusals.
fn upgrade_site(path: &str) -> String {
    format!("the {path} gateway upgrade")
}

/// The boot check `#[messages]` emits for a gateway: its upgrade chain is
/// phase-validated (an HTTP `GET`, the entry the check reads), and a guard no
/// imported module provides — on the upgrade or on any event — refuses, named
/// with its site.
pub fn check_ws_gateway(
    container: &Container,
    path: &str,
    upgrade: &[ScopedGuardSpec],
    events: &[(&'static str, Vec<ScopedGuardSpec>)],
) -> Result<(), String> {
    boot_validate_guards(container, upgrade, &upgrade_site(path))?;
    for (event, method) in events {
        resolve_scoped(container, method, LayerSite::Method, &event_site(event))
            .map_err(|unresolved| unresolved.to_string())?;
    }
    Ok(())
}

/// One event's per-message checks: the app-wide pool and the event's own
/// `#[use_guards]`, deduped unless `#[force_guards]` replays one.
pub fn ws_event_chain(
    container: &Container,
    method: &[ScopedGuardSpec],
    force: &[TypeId],
    event: &'static str,
) -> Vec<Arc<dyn WsMessageCheck>> {
    let site = event_site(event);
    let method = match resolve_scoped(container, method, LayerSite::Method, &site) {
        Ok(method) => method,
        Err(unresolved) => {
            report_unresolved(&unresolved);
            refuse_site(container, unresolved.into());
            return vec![Arc::new(RefusedEvent)];
        }
    };
    let global = dedup_bucket(resolve_global_guards(container));
    compose_chain::<dyn Guard>(global, Vec::new(), method, force, &site)
        .into_iter()
        .map(|entry| {
            Arc::new(GuardAsWsMessageCheck::new(
                entry.layer,
                entry.type_id,
                entry.name,
            )) as Arc<dyn WsMessageCheck>
        })
        .collect()
}

/// Wrap a gateway's upgrade in its own `#[use_guards]`, the first listed
/// outermost. A guard the app-wide pool also holds is left to the pool, which
/// the transport runs at the edge.
pub fn guard_ws_upgrade(
    container: &Container,
    endpoint: BoxEndpoint<'static, Response>,
    specs: &[ScopedGuardSpec],
    path: &str,
) -> BoxEndpoint<'static, Response> {
    let site = upgrade_site(path);
    let host = match resolve_scoped(container, specs, LayerSite::Host, &site) {
        Ok(host) => host,
        Err(unresolved) => {
            report_unresolved(&unresolved);
            refuse_site(container, unresolved.into());
            return poem::endpoint::make_sync(|_| refused_route()).boxed();
        }
    };
    let global = dedup_bucket(resolve_global_guards(container));
    compose_chain::<dyn Guard>(global, host, Vec::new(), &[], &site)
        .into_iter()
        .rev()
        .filter(|entry| entry.source != LayerSite::Global)
        .fold(endpoint, |inner, entry| {
            inner.guard(entry.layer).map_to_response().boxed()
        })
}

/// The check an event whose chain did not compose runs instead: it refuses
/// every message, opaquely.
struct RefusedEvent;

#[async_trait::async_trait]
impl WsMessageCheck for RefusedEvent {
    async fn check(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), WsError> {
        Err(denial_to_ws_error(Denial::internal(
            "the event's guard chain did not compose",
        )))
    }

    fn type_key(&self) -> TypeId {
        TypeId::of::<Self>()
    }

    fn layer_name(&self) -> &'static str {
        "the event's guard chain, which did not compose"
    }
}
