//! `#[hooks]` through the real macro: phase-tagged methods run dependencies
//! first at init and in reverse at shutdown, bare and `Result`-returning
//! alike; a failing init hook aborts the boot and tears every host down; the
//! harness runs them once, hands out the app's lifecycle, and closes the app,
//! stopping what it spawned before any teardown and filing a spawned
//! transport that fails.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use nest_rs_core::{App, AppLifecycle, AppState, StopCause, hooks, injectable, module, target};
use nest_rs_testing::{LogCapture, TestApp};

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
async fn hooks_run_dependencies_first_at_init_and_in_exact_reverse_when_the_harness_closes() {
    let app = TestApp::for_module::<ChainModule>().await.expect("boots");
    let lifecycle = Arc::clone(app.lifecycle());

    app.close().await;

    assert_eq!(lifecycle.state(), AppState::Draining);
    assert_eq!(lifecycle.cause(), Some(StopCause::Closed));
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

static HEADLESS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn headless(entry: &'static str) {
    HEADLESS
        .lock()
        .expect("log mutex is not poisoned")
        .push(entry);
}

/// Serves until its token is cancelled, then takes `UNWINDS` to stop and
/// records `"transport stopped"` through its log.
struct Unwinding(fn(&'static str));

const UNWINDS: std::time::Duration = std::time::Duration::from_secs(2);

#[async_trait::async_trait]
impl nest_rs_core::Transport for Unwinding {
    async fn configure(&mut self, _container: &nest_rs_core::Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(
        self: Box<Self>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        cancel.cancelled().await;
        tokio::time::sleep(UNWINDS).await;
        (self.0)("transport stopped");
        Ok(())
    }

    fn stop_bound(&self) -> std::time::Duration {
        UNWINDS + std::time::Duration::from_secs(1)
    }
}

#[injectable]
struct Drained;

#[hooks]
impl Drained {
    #[on_module_destroy]
    async fn release(&self) {
        headless("hook ran");
    }
}

#[module(providers = [Drained])]
struct DrainedModule;

#[tokio::test(start_paused = true)]
async fn a_headless_close_stops_its_transports_within_their_bound_then_runs_the_hooks() {
    use nest_rs_core::Transport as _;

    let app = TestApp::builder()
        .module::<DrainedModule>()
        .build_headless()
        .await
        .expect("boots");
    app.init().await.expect("the init phases run");
    let _serving = app
        .spawn_transport(Unwinding(headless))
        .await
        .expect("the transport configures");
    let started = tokio::time::Instant::now();

    app.close().await;

    let took = started.elapsed();
    assert!(
        took >= UNWINDS && took < Unwinding(headless).stop_bound(),
        "the close waited for the transport to stop, within its bound: took {took:?}",
    );
    assert_eq!(
        *HEADLESS.lock().expect("log mutex is not poisoned"),
        ["transport stopped", "hook ran"],
    );
}

static STOPPED_BEFORE_TEARDOWN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn stopped_before_teardown(entry: &'static str) {
    STOPPED_BEFORE_TEARDOWN
        .lock()
        .expect("log mutex is not poisoned")
        .push(entry);
}

#[injectable]
struct RefusesToWarm;

#[hooks]
impl RefusesToWarm {
    #[on_module_init]
    async fn warm(&self) -> anyhow::Result<()> {
        anyhow::bail!("the cache refused to warm")
    }

    #[on_module_destroy]
    async fn release(&self) {
        stopped_before_teardown("hook ran");
    }
}

#[module(providers = [RefusesToWarm])]
struct RefusesToWarmModule;

#[tokio::test(start_paused = true)]
async fn a_headless_init_that_fails_stops_its_transports_before_the_shutdown_hooks_run() {
    let app = TestApp::builder()
        .module::<RefusesToWarmModule>()
        .build_headless()
        .await
        .expect("boots");
    let _serving = app
        .spawn_transport(Unwinding(stopped_before_teardown))
        .await
        .expect("the transport configures");

    let error = app.init().await.expect_err("the init hook fails");

    assert!(
        format!("{error:#}").contains("the cache refused to warm"),
        "{error:#}"
    );
    assert_eq!(
        *STOPPED_BEFORE_TEARDOWN
            .lock()
            .expect("log mutex is not poisoned"),
        ["transport stopped", "hook ran"],
        "nothing the harness serves runs into the teardown",
    );
}

/// Fails as soon as it serves, as a transport whose port was taken does.
struct FailsToServe;

#[async_trait::async_trait]
impl nest_rs_core::Transport for FailsToServe {
    async fn configure(&mut self, _container: &nest_rs_core::Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(
        self: Box<Self>,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("the address was already in use")).context("the listener did not bind")
    }

    fn stop_bound(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

#[module]
struct ServesNothingModule;

#[tokio::test]
async fn a_spawned_transport_that_fails_is_filed_at_error_with_its_chain() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<ServesNothingModule>()
        .build_headless()
        .await
        .expect("boots");
    let _serving = app
        .spawn_transport(FailsToServe)
        .await
        .expect("the transport configures");

    app.close().await;

    let failed = logs.expect_one(target::APP, "a transport the harness spawned failed");
    assert_eq!(failed.level, "error");
    assert!(
        failed
            .field("transport")
            .is_some_and(|transport| transport.ends_with("FailsToServe")),
        "{:?}",
        failed.fields,
    );
    let error = failed.field("error").unwrap_or_default();
    assert!(
        error.contains("the listener did not bind")
            && error.contains("the address was already in use"),
        "the whole chain: {error}",
    );
}

/// Fails its `listen`, or states the stop it was built with.
struct Refused {
    listens: bool,
    stop_bound: std::time::Duration,
}

static REFUSED_SERVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[async_trait::async_trait]
impl nest_rs_core::Transport for Refused {
    async fn configure(&mut self, _container: &nest_rs_core::Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn listen(&mut self) -> anyhow::Result<nest_rs_core::Listening> {
        if self.listens {
            return Ok(nest_rs_core::Listening::default());
        }
        Err(anyhow::anyhow!("the address was already in use"))
    }

    async fn serve(
        self: Box<Self>,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        REFUSED_SERVED.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop_bound(&self) -> std::time::Duration {
        self.stop_bound
    }
}

/// The harness spawns a transport as `App::run` boots one: its stop bound is
/// held, and it listens before it serves.
#[tokio::test]
async fn a_spawned_transport_is_refused_an_unbounded_stop_and_listens_before_it_serves() {
    let app = TestApp::builder()
        .module::<ServesNothingModule>()
        .build_headless()
        .await
        .expect("boots");

    let unbounded = app
        .spawn_transport(Refused {
            listens: true,
            stop_bound: std::time::Duration::MAX,
        })
        .await
        .err()
        .expect("an unbounded stop is refused");
    assert!(
        unbounded
            .downcast_ref::<nest_rs_core::UnboundedTransportError>()
            .is_some(),
        "{unbounded:#}"
    );
    let unbound = app
        .spawn_transport(Refused {
            listens: false,
            stop_bound: std::time::Duration::ZERO,
        })
        .await
        .err()
        .expect("a listen that fails is the spawn's error");
    assert_eq!(unbound.to_string(), "the address was already in use");
    assert!(
        !REFUSED_SERVED.load(Ordering::SeqCst),
        "neither transport served"
    );

    app.close().await;
}

static WS_RELEASED: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct ReleasedOverWs;

#[hooks]
impl ReleasedOverWs {
    #[on_module_destroy]
    async fn release(&self) {
        WS_RELEASED.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [ReleasedOverWs])]
struct ReleasedOverWsModule;

#[tokio::test]
async fn a_socket_harness_close_stops_its_transport_then_runs_the_hooks() {
    let app = TestApp::builder()
        .module::<ReleasedOverWsModule>()
        .build_ws()
        .await
        .expect("boots on a local port");
    let addr = app.addr();

    app.close().await;

    assert_eq!(WS_RELEASED.load(Ordering::SeqCst), 1);
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "the transport no longer listens"
    );
}

mod the_harness_init_fails {
    use std::sync::Mutex;

    failed_boot_hosts!(HARNESS_FAILED_INIT, "Repo::init");
}

#[tokio::test]
async fn a_harness_whose_init_fails_tears_every_host_down_before_it_returns() {
    let Err(error) = TestApp::for_module::<the_harness_init_fails::BootModule>().await else {
        panic!("the init hook fails the harness's boot");
    };

    assert!(
        format!("{error:#}").contains("Repo::init refused"),
        "{error:#}"
    );
    assert_eq!(
        the_harness_init_fails::log(),
        [
            "Db::init",
            "Repo::init",
            "Api::destroy",
            "Svc::destroy",
            "Repo::destroy",
            "Db::destroy",
        ],
    );
}
