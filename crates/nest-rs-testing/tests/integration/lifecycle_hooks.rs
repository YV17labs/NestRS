//! `#[hooks]` through the real macro: phase-tagged methods run dependencies
//! first at init and in reverse at shutdown, bare and `Result`-returning
//! alike, and a failing init hook aborts the boot; the harness runs them once
//! and hands out the app's lifecycle.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nest_rs_core::{App, AppLifecycle, AppState, hooks, injectable, module};
use nest_rs_testing::TestApp;

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

#[module(providers = [Beta, Alpha])]
struct HooksModule;

#[tokio::test]
async fn unrelated_hosts_run_per_phase_in_the_order_their_module_lists_them() {
    let app = App::new::<HooksModule>().expect("boots");
    app.init().await.expect("init phases succeed");

    let log = LOG.lock().expect("log mutex is not poisoned").clone();
    assert_eq!(log, vec!["Beta::b_init", "Alpha::a_init", "Alpha::a_boot"]);
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

static HARNESS_INITS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct WatchesTheApp {
    #[inject]
    lifecycle: Arc<AppLifecycle>,
}

#[hooks]
impl WatchesTheApp {
    #[on_module_init]
    async fn count(&self) {
        HARNESS_INITS.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [WatchesTheApp])]
struct WatchedModule;

#[tokio::test]
async fn the_harness_runs_init_once_and_hands_out_the_lifecycle_its_providers_inject() {
    let app = TestApp::for_module::<WatchedModule>()
        .await
        .expect("the module boots");
    let watcher = app
        .container()
        .get::<WatchesTheApp>()
        .expect("the provider is built");
    assert!(Arc::ptr_eq(app.lifecycle(), &watcher.lifecycle));
    assert_eq!(app.lifecycle().state(), AppState::Starting);

    let headless = TestApp::builder()
        .module::<WatchedModule>()
        .build_headless()
        .await
        .expect("the module boots");
    headless.init().await.expect("the init phases run");
    headless.init().await.expect("a second init is a no-op");
    let watcher = headless
        .container()
        .get::<WatchesTheApp>()
        .expect("the provider is built");
    assert!(Arc::ptr_eq(headless.lifecycle(), &watcher.lifecycle));

    assert_eq!(
        HARNESS_INITS.load(Ordering::SeqCst),
        2,
        "each app ran its init hooks once: the TestApp's build, the HeadlessApp's first init"
    );
}

static CHAIN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn chained(entry: &'static str) {
    CHAIN.lock().expect("log mutex is not poisoned").push(entry);
}

#[injectable]
struct Zdb;

#[hooks]
impl Zdb {
    #[on_module_init]
    async fn init(&self) {
        chained("Zdb::init");
    }

    #[on_module_destroy]
    async fn destroy(&self) {
        chained("Zdb::destroy");
    }
}

#[injectable]
struct Mrepo {
    #[inject]
    _db: Arc<Zdb>,
}

#[hooks]
impl Mrepo {
    #[on_module_init]
    async fn init(&self) {
        chained("Mrepo::init");
    }

    #[on_module_destroy]
    async fn destroy(&self) {
        chained("Mrepo::destroy");
    }
}

#[injectable]
struct Aservice {
    #[inject]
    _repo: Arc<Mrepo>,
}

#[hooks]
impl Aservice {
    #[on_module_init]
    async fn init(&self) {
        chained("Aservice::init");
    }

    #[on_module_destroy]
    async fn destroy(&self) {
        chained("Aservice::destroy");
    }
}

#[module(providers = [Aservice, Mrepo, Zdb])]
struct ChainModule;

#[tokio::test]
async fn hooks_run_dependencies_first_at_init_and_in_exact_reverse_at_shutdown() {
    App::new::<ChainModule>()
        .expect("boots")
        .run()
        .await
        .expect("an app serving nothing stops at once");

    let log = CHAIN.lock().expect("log mutex is not poisoned").clone();
    assert_eq!(
        log,
        [
            "Zdb::init",
            "Mrepo::init",
            "Aservice::init",
            "Aservice::destroy",
            "Mrepo::destroy",
            "Zdb::destroy",
        ],
        "named against the alphabet and listed against the dependencies, the injections decide",
    );
}

/// Hosts `Db ← Repo ← Svc` and an unrelated `Api`, each with an init and a
/// destroy hook, recording into `$log`; `$fails` names the one hook that fails.
macro_rules! failed_boot_hosts {
    ($log:ident, $fails:literal) => {
        static $log: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

        fn ran(entry: &'static str) -> anyhow::Result<()> {
            $log.lock().expect("log mutex is not poisoned").push(entry);
            if entry == $fails {
                anyhow::bail!("{entry} refused");
            }
            Ok(())
        }

        #[nest_rs_core::injectable]
        pub(super) struct Db;

        #[nest_rs_core::hooks]
        impl Db {
            #[on_module_init]
            async fn init(&self) -> anyhow::Result<()> {
                ran("Db::init")
            }

            #[on_module_destroy]
            async fn destroy(&self) -> anyhow::Result<()> {
                ran("Db::destroy")
            }
        }

        #[nest_rs_core::injectable]
        pub(super) struct Repo {
            #[inject]
            _db: std::sync::Arc<Db>,
        }

        #[nest_rs_core::hooks]
        impl Repo {
            #[on_module_init]
            async fn init(&self) -> anyhow::Result<()> {
                ran("Repo::init")
            }

            #[on_application_bootstrap]
            async fn bootstrap(&self) -> anyhow::Result<()> {
                ran("Repo::bootstrap")
            }

            #[on_module_destroy]
            async fn destroy(&self) -> anyhow::Result<()> {
                ran("Repo::destroy")
            }
        }

        #[nest_rs_core::injectable]
        pub(super) struct Svc {
            #[inject]
            _repo: std::sync::Arc<Repo>,
        }

        #[nest_rs_core::hooks]
        impl Svc {
            #[on_module_init]
            async fn init(&self) -> anyhow::Result<()> {
                ran("Svc::init")
            }

            #[on_module_destroy]
            async fn destroy(&self) -> anyhow::Result<()> {
                ran("Svc::destroy")
            }
        }

        #[nest_rs_core::injectable]
        pub(super) struct Api;

        #[nest_rs_core::hooks]
        impl Api {
            #[on_module_init]
            async fn init(&self) -> anyhow::Result<()> {
                ran("Api::init")
            }

            #[on_module_destroy]
            async fn destroy(&self) -> anyhow::Result<()> {
                ran("Api::destroy")
            }
        }

        #[nest_rs_core::module(providers = [Svc, Repo, Db, Api])]
        pub(super) struct BootModule;

        pub(super) fn log() -> Vec<&'static str> {
            $log.lock().expect("log mutex is not poisoned").clone()
        }
    };
}

mod an_init_hook_fails {
    use std::sync::Mutex;

    failed_boot_hosts!(FAILED_INIT, "Svc::init");
}

#[tokio::test]
async fn a_failed_init_hook_runs_every_hosts_destroy_hook_in_reverse_and_run_returns_its_error() {
    let error = App::new::<an_init_hook_fails::BootModule>()
        .expect("boots")
        .run()
        .await
        .expect_err("the init hook's error is the run's");

    assert!(
        format!("{error:#}").contains("Svc::init refused"),
        "{error:#}"
    );
    assert_eq!(
        an_init_hook_fails::log(),
        [
            "Db::init",
            "Repo::init",
            "Svc::init",
            "Api::destroy",
            "Svc::destroy",
            "Repo::destroy",
            "Db::destroy",
        ],
        "every present host is torn down, `Api` included, its init never having run",
    );
}

mod a_bootstrap_hook_fails {
    use std::sync::Mutex;

    failed_boot_hosts!(FAILED_BOOTSTRAP, "Repo::bootstrap");
}

#[tokio::test]
async fn a_failed_bootstrap_hook_runs_every_hosts_destroy_hook_in_reverse() {
    let error = App::new::<a_bootstrap_hook_fails::BootModule>()
        .expect("boots")
        .run()
        .await
        .expect_err("the bootstrap hook's error is the run's");

    assert!(
        format!("{error:#}").contains("Repo::bootstrap refused"),
        "{error:#}"
    );
    assert_eq!(
        a_bootstrap_hook_fails::log(),
        [
            "Db::init",
            "Repo::init",
            "Svc::init",
            "Api::init",
            "Repo::bootstrap",
            "Api::destroy",
            "Svc::destroy",
            "Repo::destroy",
            "Db::destroy",
        ],
    );
}
