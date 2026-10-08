//! Dispatch order: providers in `providers = [...]` order, then methods in
//! `#[listeners]` block order — never `inventory`'s link order.

use std::sync::Arc;

use nest_rs_core::{App, injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};
use parking_lot::Mutex;

#[derive(Clone)]
struct Ping;

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

// Deliberately not in alphabetical order.
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

#[tokio::test]
async fn the_order_is_stable_across_boots() {
    let first = dispatch_order().await;
    let second = dispatch_order().await;
    assert_eq!(first, second);
}
