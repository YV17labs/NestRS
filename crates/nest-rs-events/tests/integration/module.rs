//! `src/module.rs` — when `EventsModule` wires the listeners: once, by the
//! kernel's wiring step, before the first lifecycle hook.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, hooks, injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};

#[derive(Clone)]
struct Seeded {
    rows: usize,
}

#[injectable]
#[derive(Default)]
struct SeedLog {
    rows: AtomicUsize,
}

#[injectable]
struct SeedListener {
    #[inject]
    log: Arc<SeedLog>,
}

#[listeners]
impl SeedListener {
    #[on_event]
    async fn on_seeded(&self, event: Seeded) {
        self.log.rows.fetch_add(event.rows, Ordering::SeqCst);
    }
}

/// Seeds at `OnModuleInit` and says so on the bus, as a seeding service does.
#[injectable]
struct Seeder {
    #[inject]
    bus: Arc<EventBus>,
}

#[hooks]
impl Seeder {
    #[on_module_init]
    async fn seed(&self) {
        self.bus.emit(Seeded { rows: 3 }).await;
    }
}

#[module(imports = [EventsModule], providers = [SeedLog, SeedListener, Seeder])]
struct SeedingModule;

#[tokio::test]
async fn an_event_emitted_by_an_init_hook_reaches_its_listener() {
    let app = App::builder()
        .module::<SeedingModule>()
        .build()
        .await
        .expect("boots");
    app.init().await.expect("the init hooks run");

    let log = app
        .container()
        .get::<SeedLog>()
        .expect("SeedLog is provided");
    assert_eq!(
        log.rows.load(Ordering::SeqCst),
        3,
        "the listener was subscribed before the first hook, so the hook's event reached it",
    );
}

#[tokio::test]
async fn the_synchronous_boot_wires_the_listeners_too() {
    let app = App::new::<SeedingModule>().expect("boots");
    let bus = app
        .container()
        .get::<EventBus>()
        .expect("the bus is provided");

    bus.emit(Seeded { rows: 2 }).await;

    let log = app
        .container()
        .get::<SeedLog>()
        .expect("SeedLog is provided");
    assert_eq!(
        log.rows.load(Ordering::SeqCst),
        2,
        "no hook has run, and the listener is live",
    );
}

#[tokio::test]
async fn an_emit_on_a_bus_no_wiring_step_reached_is_dropped_with_a_warn() {
    // A container built by hand runs no kernel step, so its bus is never wired.
    let container = nest_rs_core::Container::builder()
        .import::<SeedingModule>()
        .build();
    let bus = container.get::<EventBus>().expect("the bus is provided");
    let logs = nest_rs_testing::LogCapture::install();

    bus.emit(Seeded { rows: 5 }).await;

    let event = logs.expect_one(
        nest_rs_events::TARGET,
        "event emitted before the listeners were wired: dropped",
    );
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("event")
            .is_some_and(|name| name.ends_with("Seeded")),
        "the line names the event type, got {:?}",
        event.fields,
    );
    let log = container.get::<SeedLog>().expect("SeedLog is provided");
    assert_eq!(log.rows.load(Ordering::SeqCst), 0);
}
