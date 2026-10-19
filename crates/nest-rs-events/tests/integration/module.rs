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
        .build()
        .expect("the container builds");
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

/// A listener that emits holds the bus the bus holds it on: the app's end
/// unsubscribes it, so both drop with the app.
mod dropped_at_the_end {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nest_rs_core::{App, injectable, module};
    use nest_rs_events::{EventBus, EventsModule, listeners};

    static DROPPED: AtomicBool = AtomicBool::new(false);

    #[derive(Clone)]
    struct Paid;

    #[derive(Clone)]
    struct Receipted;

    #[injectable]
    struct Receipts {
        #[inject]
        bus: Arc<EventBus>,
    }

    impl Drop for Receipts {
        fn drop(&mut self) {
            DROPPED.store(true, Ordering::SeqCst);
        }
    }

    #[listeners]
    impl Receipts {
        #[on_event]
        async fn on_paid(&self, _event: Paid) {
            self.bus.emit(Receipted).await;
        }
    }

    #[module(imports = [EventsModule], providers = [Receipts])]
    struct ReceiptsModule;

    #[tokio::test]
    async fn a_listener_that_emits_is_dropped_when_the_app_closes() {
        let app = App::builder()
            .module::<ReceiptsModule>()
            .build()
            .await
            .expect("boots");

        app.close().await;

        assert!(
            DROPPED.load(Ordering::SeqCst),
            "the bus let go of the listener holding it"
        );
    }
}

/// A value whose expression hides its type from `#[listeners]`.
struct Tag;

impl Tag {
    fn new() -> Self {
        Self
    }
}

#[derive(Clone)]
struct Audited;

#[injectable]
#[derive(Default)]
struct TaggedTwiceListeners;

#[listeners]
#[meta(Tag::new())]
#[meta(Tag::new())]
impl TaggedTwiceListeners {
    #[on_event]
    async fn on_audited(&self, _event: Audited) {}
}

#[module(imports = [EventsModule], providers = [TaggedTwiceListeners])]
struct TaggedTwiceModule;

#[tokio::test]
async fn two_values_of_one_hidden_type_on_a_listener_host_fail_the_boot_naming_it() {
    let Err(refused) = App::builder().module::<TaggedTwiceModule>().build().await else {
        panic!("the listener would run with a value read by nothing");
    };
    let chain = format!("{refused:#}");
    assert!(
        chain.contains("module::Tag`") && chain.contains("on `TaggedTwiceListeners`"),
        "the refusal names the type and the host declaring it twice: {chain}",
    );
}
