//! Dispatch order — the guarantee the events page states:
//!
//! > "**Order is deterministic** — registration order is preservation order;
//! > listeners are registered in the order their providers appear in
//! > `providers = [...]`, then in the order their methods appear in the
//! > `#[listeners]` impl block."
//!
//! Both halves used to fail. `inventory` hands entries back in **link order**,
//! which is stable across restarts of the same binary and reshuffles whenever
//! the code changes — the worst possible shape for a guarantee, because a
//! developer orders two listeners deliberately, verifies it locally, and gets a
//! silent rearrangement the next time somebody adds an unrelated third to the
//! same block. Three methods declared `first, second, third` dispatched
//! `2, 3, 1`; a second provider's listener landed *between* two of the first
//! provider's, so they were not even grouped by provider.
//!
//! The bus faithfully preserves whatever the registry hands it, so the ordering
//! is restored upstream: each `#[on_event]` submits its position in its block,
//! and `EventsModule` sorts on (provider rank in the module walk, that index).

use std::sync::Arc;

use nest_rs_core::{App, injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};
use parking_lot::Mutex;

#[derive(Clone)]
struct Ping;

/// Shared sink so the order is observed as one sequence across both providers.
#[injectable]
#[derive(Default)]
struct Trace {
    seen: Mutex<Vec<&'static str>>,
}

impl Trace {
    fn record(&self, who: &'static str) {
        self.seen.lock().push(who);
    }

    fn seen(&self) -> Vec<&'static str> {
        self.seen.lock().clone()
    }
}

#[injectable]
struct FirstProvider {
    #[inject]
    trace: Arc<Trace>,
}

// Declared first, second, third — and deliberately *not* in alphabetical or
// link order, so a sort that happens to agree by accident would not pass.
#[listeners]
impl FirstProvider {
    #[on_event]
    async fn zulu(&self, _event: Ping) {
        self.trace.record("a.zulu");
    }

    #[on_event]
    async fn mike(&self, _event: Ping) {
        self.trace.record("a.mike");
    }

    #[on_event]
    async fn alpha(&self, _event: Ping) {
        self.trace.record("a.alpha");
    }
}

#[injectable]
struct SecondProvider {
    #[inject]
    trace: Arc<Trace>,
}

#[listeners]
impl SecondProvider {
    #[on_event]
    async fn only(&self, _event: Ping) {
        self.trace.record("b.only");
    }
}

// The declaration the guarantee refers to: `FirstProvider` before
// `SecondProvider`.
#[module(
    imports = [EventsModule],
    providers = [Trace, FirstProvider, SecondProvider],
)]
struct OrderTestModule;

async fn dispatch_order() -> Vec<&'static str> {
    let app = App::new::<OrderTestModule>().expect("boots");
    app.init().await.expect("bootstrap wiring succeeds");
    let bus = app.container().get::<EventBus>().expect("bus");
    bus.emit(Ping).await;
    app.container().get::<Trace>().expect("trace").seen()
}

#[tokio::test]
async fn listeners_dispatch_in_declaration_order() {
    assert_eq!(
        dispatch_order().await,
        vec!["a.zulu", "a.mike", "a.alpha", "b.only"],
        "methods in block order, providers grouped in `providers = [...]` order",
    );
}

/// "Deterministic" was already true of link order — stable across restarts of
/// one binary — and that is exactly what made the guarantee a trap. Re-running
/// proves stability, which only means something alongside the test above.
#[tokio::test]
async fn the_order_is_stable_across_boots() {
    let first = dispatch_order().await;
    let second = dispatch_order().await;
    assert_eq!(first, second);
}
