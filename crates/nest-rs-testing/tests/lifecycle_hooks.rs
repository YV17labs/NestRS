//! `#[hooks]` exercised through the **real** macro (not a hand-written thunk):
//! phase-tagged `async` methods are submitted to the lifecycle inventory, run
//! in `(provider, method)` order within a phase, and both the bare and
//! `Result`-returning forms are adapted to the runner's `anyhow::Result<()>`.
//!
//! The home-crate unit test in `nest-rs-core/src/lifecycle.rs` drives a
//! hand-built `LifecycleHook`; this is the cross-crate wiring test CLAUDE.md's
//! *Testing* section calls for ("hook ordering"), proving the macro's
//! `inventory::submit!`, `present` probe, and return adaptation all hold.

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
    // Bare (infallible) form — the macro adapts the `()` return to `Ok(())`.
    #[on_module_init]
    async fn a_init(&self) {
        record("Alpha::a_init");
    }

    // `Result`-returning form — the macro maps the error via `Into`.
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

    // `OnModuleInit` runs before `OnApplicationBootstrap`; within a phase,
    // entries run in `(provider, method)` name order — "Alpha" before "Beta".
    // If the macro failed to submit any hook, or ran one against a missing
    // provider, this exact sequence would not appear.
    let log = LOG.lock().expect("log mutex is not poisoned").clone();
    assert_eq!(log, vec!["Alpha::a_init", "Beta::b_init", "Alpha::a_boot"]);
}

// --- init-hook failure aborts boot --------------------------------------------
//
// The init runner (`run_phase`) is sequential and aborts on the first error, so
// a provider whose `#[on_module_init]` returns `Err` must fail `App::init` (and,
// equivalently, `App::run` before it starts serving — nothing is listening yet).
// The failing-*factory* case is pinned in
// `nest-rs-core/src/app.rs::factory_error_aborts_build`; this covers the failing
// *hook* case, through the real `#[hooks]` macro rather than a hand-written thunk.
//
// `FailingInit`'s hook lands in the same process-global inventory as `Alpha` /
// `Beta`, but `#[hooks]` gates each hook on a `Container::get::<Provider>()`
// probe, so booting `HooksModule` (which never lists `FailingInit`) skips it —
// the test above stays green.

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
    // The container builds fine — hooks run at `init`, not at `new`.
    let app = App::new::<FailingInitModule>().expect("the container builds");

    // `App` is not `Debug`, but `init` yields `Result<(), _>` (and `()` is
    // `Debug`), so `expect_err` is available.
    let err = app
        .init()
        .await
        .expect_err("a failing init hook must abort boot");

    // `run_phase` wraps the hook error with the hook's identity and phase, and
    // `{:#}` walks the source chain down to the hook's own message.
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
