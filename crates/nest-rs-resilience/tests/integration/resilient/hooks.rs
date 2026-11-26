//! A decorated hook of a `#[hooks]` block, attempted again within the one run
//! the app's init makes, and refused: an init hook failing the boot, a shutdown
//! hook filed with the hook after it run.

use std::sync::Arc;

use std::sync::atomic::{AtomicU32, Ordering};

use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
use nest_rs_core::{BootStep, LifecyclePhase, hooks, injectable, module};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{HeadlessApp, LogCapture};

use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

#[injectable]
struct CacheWarmer {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[hooks]
impl CacheWarmer {
    #[on_module_init]
    #[resilient(upstream, idempotent)]
    async fn warm(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(())
    }
}

#[module(
    imports = [ResilienceModule::for_root(retrying_once())],
    providers = [CacheWarmer, Attempts],
)]
struct DecoratedHooksModule;

#[tokio::test(start_paused = true)]
async fn a_hook_failing_once_answers_on_its_retry_and_the_init_completes() {
    let app = HeadlessApp::builder()
        .module::<DecoratedHooksModule>()
        .build_headless()
        .await
        .expect("a decorated hook boots");
    app.init().await.expect("the hook answered on its retry");

    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
}

#[injectable]
struct WarmingHooks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[hooks]
impl WarmingHooks {
    #[on_module_init]
    #[resilient(upstream, timeout = "100ms")]
    async fn warm(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        self.dependency.call().await
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [WarmingHooks, Dependency],
)]
struct RefusedInitModule;

/// No client reads an init hook's `503`: its refusal fails the boot, the
/// error naming the hook over the refusal itself, which a `main` matches.
#[tokio::test(start_paused = true)]
async fn an_init_hook_its_policy_refuses_fails_the_boot_naming_it() {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<RefusedInitModule>()
            .build_headless()
            .await
            .expect("a decorated hook boots");
        let hooks = app.container().get::<WarmingHooks>().expect("provided");
        let (upstream, dependency) = refusing.arrange(app.container());
        let mut events = upstream.events();

        let failed = app
            .init()
            .await
            .expect_err("the hook's refusal fails the boot");
        let told = crate::told(&mut events);
        assert_eq!(dependency.called(), refusing.sent());

        let ran = logs.expect_one(OPERATIONS, nest_rs_core::unit::HOOK.name());
        assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
        assert_eq!(
            ran.field("phase").as_deref(),
            Some(LifecyclePhase::OnModuleInit.as_str())
        );
        assert_eq!(ran.field("host").as_deref(), Some("WarmingHooks"));
        assert_eq!(ran.field("method").as_deref(), Some("warm"));
        assert_eq!(ran.field("origin").as_deref(), Some(module_path!()));
        refusing.told(&told, "WarmingHooks", "warm");
        let torn_down = logs.expect_one(
            nest_rs_core::target::LIFECYCLE,
            "the boot failed after init hooks ran: running the shutdown hooks",
        );
        assert_eq!(torn_down.field("host"), ran.field("host"), "{torn_down:?}");
        assert_eq!(torn_down.field("method"), ran.field("method"));

        assert!(
            failed
                .chain()
                .any(|link| link.is::<ResilienceError<UpstreamDown>>()),
            "the refusal itself, for a `main` to match: {failed:?}"
        );
        let refusal = failed
            .chain()
            .find(|link| link.is::<ResilienceError<UpstreamDown>>())
            .map(nest_rs_core::error_message)
            .expect("the refusal is a link of the chain");
        assert!(
            logs.events().iter().all(|line| {
                line.field("error")
                    .is_none_or(|error| !error.contains(&refusal))
            }),
            "the chain rides the error `init` returns, and no line: {:?}",
            logs.events()
        );
        let step = BootStep::InitHook {
            phase: LifecyclePhase::OnModuleInit,
            host: "WarmingHooks",
            method: "warm",
            origin: module_path!(),
        };
        let chain = refusing.chain(hooks.warm().await);
        assert_eq!(format!("{failed:#}"), format!("{step} failed: {chain}"));
    }
}

#[injectable]
struct FlushingHooks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
    closed: AtomicU32,
}

#[hooks]
impl FlushingHooks {
    #[on_module_destroy]
    #[resilient(upstream, timeout = "100ms")]
    async fn flush(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        self.dependency.call().await
    }

    #[on_application_shutdown]
    async fn close(&self) -> anyhow::Result<()> {
        self.closed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [FlushingHooks, Dependency],
)]
struct RefusedShutdownModule;

/// No client reads a shutdown hook's `503`: its refusal is filed as any
/// failure of a shutdown hook, and the hook after it still runs.
#[tokio::test(start_paused = true)]
async fn a_shutdown_hook_its_policy_refuses_is_filed_and_the_hook_after_it_runs() {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<RefusedShutdownModule>()
            .build_headless()
            .await
            .expect("a decorated hook boots");
        app.init().await.expect("no init hook fails");
        let hooks = app.container().get::<FlushingHooks>().expect("provided");
        let (upstream, dependency) = refusing.arrange(app.container());
        let mut events = upstream.events();

        app.close().await;
        let told = crate::told(&mut events);
        assert_eq!(dependency.called(), refusing.sent());

        let failed = logs.expect_one(nest_rs_core::target::LIFECYCLE, "lifecycle hook failed");
        assert_eq!(failed.level, "error", "{failed:?}");
        assert_eq!(logs.events_at(tracing::Level::ERROR).len(), 1);
        let mut ran = logs.find(OPERATIONS, nest_rs_core::unit::HOOK.name());
        ran.retain(|line| line.field("host").as_deref() == Some("FlushingHooks"));
        let [flushed, closed] = &ran[..] else {
            panic!("the refused hook, then the one after it: {ran:#?}");
        };
        assert_eq!(flushed.field("outcome").as_deref(), Some(ERROR));
        for line in [&failed, flushed] {
            assert_eq!(
                line.field("phase").as_deref(),
                Some(LifecyclePhase::OnModuleDestroy.as_str()),
                "{line:?}"
            );
            assert_eq!(line.field("host").as_deref(), Some("FlushingHooks"));
            assert_eq!(line.field("method").as_deref(), Some("flush"));
            assert_eq!(line.field("origin").as_deref(), Some(module_path!()));
        }
        assert_eq!(closed.field("method").as_deref(), Some("close"));
        assert_eq!(closed.field("outcome").as_deref(), Some(OK));
        assert_eq!(hooks.closed.load(Ordering::Relaxed), 1);
        refusing.told(&told, "FlushingHooks", "flush");

        let chain = refusing.chain(hooks.flush().await);
        assert_eq!(
            failed.field("error").as_deref(),
            Some(&*chain),
            "{failed:?}"
        );
    }
}
