//! `#[hooks]` through the real macro: phase-tagged methods run in
//! `(provider, method)` order within a phase, bare and `Result`-returning
//! alike, and a failing init hook aborts the boot.

use std::sync::Mutex;

use nest_rs_core::{App, hooks, injectable, module};

static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn record(entry: &'static str) {
    LOG.lock().expect("log mutex is not poisoned").push(entry);
}

#[injectable]
struct Alpha;

#[hooks]
impl Alpha {
    #[on_module_init]
    async fn a_init(&self) {
        record("Alpha::a_init");
    }

    #[on_application_bootstrap]
    async fn a_boot(&self) -> anyhow::Result<()> {
        record("Alpha::a_boot");
        Ok(())
    }
}

#[injectable]
struct Beta;

#[hooks]
impl Beta {
    #[on_module_init]
    async fn b_init(&self) -> anyhow::Result<()> {
        record("Beta::b_init");
        Ok(())
    }
}

#[module(providers = [Alpha, Beta])]
struct HooksModule;

#[tokio::test]
async fn hooks_run_per_phase_in_provider_method_order() {
    let app = App::new::<HooksModule>().expect("boots");
    app.init().await.expect("init phases succeed");

    let log = LOG.lock().expect("log mutex is not poisoned").clone();
    assert_eq!(log, vec!["Alpha::a_init", "Beta::b_init", "Alpha::a_boot"]);
}

// `FailingInit`'s hook shares the inventory with `Alpha`'s, but `#[hooks]` gates
// each on its provider being present, so `HooksModule` skips it.

#[injectable]
struct FailingInit;

#[hooks]
impl FailingInit {
    #[on_module_init]
    async fn boom(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("init exploded"))
    }
}

#[module(providers = [FailingInit])]
struct FailingInitModule;

#[tokio::test]
async fn a_failing_init_hook_aborts_boot() {
    let app = App::new::<FailingInitModule>().expect("the container builds");

    let err = app
        .init()
        .await
        .expect_err("a failing init hook must abort boot");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("FailingInit::boom"),
        "the error names the failing hook: {msg}",
    );
    assert!(
        msg.contains("init exploded"),
        "the error surfaces the hook's own message: {msg}",
    );
}
