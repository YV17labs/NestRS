//! `#[gateway]` + `#[use_guards]` + `#[messages]` — exercises the guard-layer
//! emission (`::nest_rs_ws::tracing`, part of the M1 regression) and the
//! message dispatch table.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::Guard;
use nest_rs_ws::{gateway, messages};

/// No-op guard: every `check_*` inherits the trait's `Ok(())` default.
#[injectable]
pub struct HygieneWsGuard;

impl Layer for HygieneWsGuard {}

impl Guard for HygieneWsGuard {}

/// Minimal gateway consumer, guarded so the `#[use_guards]` wrap is emitted.
#[gateway(path = "/hygiene")]
#[use_guards(HygieneWsGuard)]
pub struct HygieneGateway;

#[messages]
impl HygieneGateway {
    /// Payload-less, reply-less handler — the smallest legal shape.
    #[subscribe_message("hygiene.ping")]
    async fn ping(&self) {}
}
