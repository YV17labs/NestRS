//! `#[gateway]` + `#[use_guards]` + `#[messages]` — exercises the guard-layer
//! emission (`::nest_rs_ws::tracing`, part of the M1 regression), the message
//! dispatch table, and the versioned mount (`::nest_rs_ws::nest_rs_http::
//! version_path`, which a use site declaring only the umbrella must never have
//! to name).

use nest_rs::core::{Layer, injectable};
use nest_rs::guards::{Denial, Guard, HttpGuard, async_trait};
use nest_rs::http::poem::Request as HttpRequest;
use nest_rs::ws::{gateway, messages};

/// Bound on the gateway *struct*, so it runs on the upgrade — an HTTP `GET` —
/// and the attestation it owes is [`HttpGuard`], reachable through the umbrella
/// like every other path this crate proves.
///
/// It overrides `check_http` rather than inheriting the default, and that is not
/// decoration: an empty `impl Guard for X {}` beside an `impl HttpGuard` is an
/// attestation that lies, which is exactly what the trybuild snapshots next door
/// exist to refuse. The witness must not ship the shape it witnesses against.
#[injectable]
pub struct HygieneWsGuard;

impl Layer for HygieneWsGuard {}

#[async_trait]
impl Guard for HygieneWsGuard {
    async fn check_http(&self, _req: &mut HttpRequest) -> Result<(), Denial> {
        Ok(())
    }
}

impl HttpGuard for HygieneWsGuard {}

/// Minimal gateway consumer, guarded so the `#[use_guards]` wrap is emitted.
#[gateway(path = "/hygiene")]
#[use_guards(HygieneWsGuard)]
pub struct HygieneGateway;

#[messages]
impl HygieneGateway {
    /// Payload-less, reply-less handler — the smallest legal shape.
    #[subscribe_message("hygiene.ping")]
    #[public]
    async fn ping(&self) {}
}

/// The versioned mount. `version_path` lives in `nest-rs-http`, which this crate
/// does not declare — the expansion reaches it through the umbrella, so a
/// gateway that versions its address still costs the developer one dependency.
#[gateway(path = "/hygiene", version = "1")]
pub struct HygieneVersionedGateway;

#[messages]
impl HygieneVersionedGateway {
    #[subscribe_message("hygiene.ping")]
    #[public]
    async fn ping(&self) {}
}
