//! Covers `src/wiring.rs` — the kernel's wiring step: what a module attaches
//! with `provide_wiring` runs once per app, against the sealed container,
//! before any lifecycle hook, in registration order; the first failure fails
//! the boot naming its registry.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::anyhow;
use nest_rs_core::{
    App, Container, ContainerBuilder, Discovery, Module, ReachableProviders, Registering,
    WiringContribution, WiringFailedError, hooks, injectable, module,
};

const LEDGER: &str = "tests::ledger";

static LEDGER_HOOK_RAN: AtomicBool = AtomicBool::new(false);

#[injectable]
#[derive(Default)]
struct LedgerReader;

#[hooks]
impl LedgerReader {
    #[on_module_init]
    async fn read(&self) {
        LEDGER_HOOK_RAN.store(true, Ordering::SeqCst);
    }
}

fn fill_ledger(_: &Container) -> anyhow::Result<()> {
    Err(anyhow!("the ledger table is missing"))
}

struct LedgerWiring;

impl Module for LedgerWiring {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_wiring(LEDGER, fill_ledger)
    }
}

#[module(imports = [LedgerWiring], providers = [LedgerReader])]
struct LedgerModule;

fn the_wiring_refusal(refused: &anyhow::Error) -> &WiringFailedError {
    refused
        .downcast_ref::<WiringFailedError>()
        .unwrap_or_else(|| panic!("not the wiring refusal: {refused:#}"))
}

#[tokio::test]
async fn a_wiring_that_fails_fails_the_boot_naming_its_registry() {
    let Err(built) = App::builder().module::<LedgerModule>().build().await else {
        panic!("a registry the app reads would be left empty");
    };
    let Err(synchronous) = App::new::<LedgerModule>() else {
        panic!("a registry the app reads would be left empty");
    };
    for refused in [built, synchronous] {
        let wiring = the_wiring_refusal(&refused);
        assert_eq!(wiring.registry, LEDGER);
        let chain = format!("{refused:#}");
        assert!(
            chain.contains(LEDGER),
            "the refusal names the registry: {chain}"
        );
        assert!(
            chain.contains("the ledger table is missing"),
            "…and carries the wiring's own error as its cause: {chain}",
        );
    }
    assert!(
        !LEDGER_HOOK_RAN.load(Ordering::SeqCst),
        "no hook runs on a boot whose wiring failed",
    );
}

static WIRED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn wired() -> Vec<&'static str> {
    WIRED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn record(name: &'static str) {
    WIRED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(name);
}

struct FirstWiring;

impl Module for FirstWiring {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_wiring("tests::first", |container| {
            // The seal's seeds are present: the step reads the final container.
            if container.get::<ReachableProviders>().is_none() {
                return Err(anyhow!("the wiring ran before the seal"));
            }
            record("tests::first");
            Ok(())
        })
    }
}

struct SecondWiring;

impl Module for SecondWiring {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_wiring("tests::second", |_| {
            record("tests::second");
            Ok(())
        })
    }
}

#[module(imports = [FirstWiring, SecondWiring])]
struct OrderedModule;

#[tokio::test]
async fn wirings_run_once_each_in_registration_order_against_the_sealed_container() {
    let app = App::builder()
        .module::<OrderedModule>()
        .build()
        .await
        .expect("boots");
    assert_eq!(wired(), ["tests::first", "tests::second"]);

    app.init().await.expect("the init hooks run");
    app.init().await.expect("and run again");
    assert_eq!(
        wired(),
        ["tests::first", "tests::second"],
        "the init phases wire nothing: the step ran once, at the boot",
    );

    let names: Vec<&'static str> = Discovery::new(app.container())
        .meta::<WiringContribution>()
        .iter()
        .map(|contribution| contribution.meta.name())
        .collect();
    assert_eq!(
        names,
        ["tests::first", "tests::second"],
        "each attached wiring is readable, in registration order",
    );
}
