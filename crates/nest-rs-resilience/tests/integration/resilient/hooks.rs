//! A decorated hook of a `#[hooks]` block, attempted again within the one run
//! the app's init makes, and refused in each of the five phases: an init hook
//! failing the boot, a shutdown hook filed with the hook after it run.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
use nest_rs_core::{BootStep, Container, LifecyclePhase, Module, hooks, injectable, module};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{HeadlessApp, LogCapture};

use super::{Attempts, Dependency, Refusable, Refusing, UpstreamDown, refusing, retrying_once};

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
    async fn warm(&self) -> Refusable {
        self.dependency.call().await
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [WarmingHooks, Dependency],
)]
struct RefusedInitModule;

#[injectable]
struct AnnouncingHooks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[hooks]
impl AnnouncingHooks {
    #[on_application_bootstrap]
    #[resilient(upstream, timeout = "100ms")]
    async fn announce(&self) -> Refusable {
        self.dependency.call().await
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [AnnouncingHooks, Dependency],
)]
struct RefusedBootstrapModule;

/// The decorated hook `M` declares, `method` of `host` in the init `phase`,
/// refused each way: its refusal fails the boot, the error naming the hook
/// over the refusal itself, which a `main` matches. `called` is that hook
/// called as its own caller calls it.
async fn an_init_hook_refused_fails_the_boot<M: Module + 'static>(
    phase: LifecyclePhase,
    (host, method): (&'static str, &'static str),
    called: impl AsyncFn(&Container) -> Refusable,
) {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<M>()
            .build_headless()
            .await
            .expect("a decorated hook boots");
        let refused = refusing.arrange(app.container()).await;
        let (upstream, dependency) = (&refused.upstream, &refused.dependency);
        let mut events = upstream.events();

        let failed = app
            .init()
            .await
            .expect_err("the hook's refusal fails the boot");
        let told = crate::told(&mut events);
        assert_eq!(dependency.called(), refusing.sent());

        let mut ran = logs.find(OPERATIONS, nest_rs_core::unit::HOOK.name());
        ran.retain(|line| line.field("host").as_deref() == Some(host));
        let [ran] = &ran[..] else {
            panic!("the refused hook, once: {ran:#?}");
        };
        assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
        assert_eq!(ran.field("phase").as_deref(), Some(phase.as_str()));
        assert_eq!(ran.field("host").as_deref(), Some(host));
        assert_eq!(ran.field("method").as_deref(), Some(method));
        assert_eq!(ran.field("origin").as_deref(), Some(module_path!()));
        refusing.told(&told, host, method);
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
            phase,
            host,
            method,
            origin: module_path!(),
        };
        let chain = refusing.chain(called(app.container()).await);
        assert_eq!(format!("{failed:#}"), format!("{step} failed: {chain}"));
    }
}

/// No client reads an init hook's `503`, in the first init phase.
#[tokio::test(start_paused = true)]
async fn an_init_hook_its_policy_refuses_fails_the_boot_naming_it() {
    an_init_hook_refused_fails_the_boot::<RefusedInitModule>(
        LifecyclePhase::OnModuleInit,
        ("WarmingHooks", "warm"),
        async |container| hooks_of::<WarmingHooks>(container).warm().await,
    )
    .await;
}

/// Nor in the second, once every module has initialized.
#[tokio::test(start_paused = true)]
async fn a_bootstrap_hook_its_policy_refuses_fails_the_boot_naming_it() {
    an_init_hook_refused_fails_the_boot::<RefusedBootstrapModule>(
        LifecyclePhase::OnApplicationBootstrap,
        ("AnnouncingHooks", "announce"),
        async |container| hooks_of::<AnnouncingHooks>(container).announce().await,
    )
    .await;
}

/// The host of hooks `H` `container` holds.
fn hooks_of<H: Send + Sync + 'static>(container: &Container) -> Arc<H> {
    container.get::<H>().expect("provided")
}

/// How many times an undecorated hook after a refused one ran.
#[injectable]
#[derive(Default)]
struct Closes {
    made: AtomicU32,
}

#[injectable]
struct FlushingHooks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
    #[inject]
    closes: Arc<Closes>,
}

#[hooks]
impl FlushingHooks {
    #[on_module_destroy]
    #[resilient(upstream, timeout = "100ms")]
    async fn flush(&self) -> Refusable {
        self.dependency.call().await
    }

    #[on_application_shutdown]
    async fn close(&self) -> anyhow::Result<()> {
        self.closes.made.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [FlushingHooks, Dependency, Closes],
)]
struct RefusedShutdownModule;

#[injectable]
struct DrainingHooks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
    #[inject]
    closes: Arc<Closes>,
}

#[hooks]
impl DrainingHooks {
    #[before_application_shutdown]
    #[resilient(upstream, timeout = "100ms")]
    async fn drain(&self) -> Refusable {
        self.dependency.call().await
    }

    #[on_application_shutdown]
    async fn close(&self) -> anyhow::Result<()> {
        self.closes.made.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [DrainingHooks, Dependency, Closes],
)]
struct RefusedBeforeShutdownModule;

#[injectable]
struct ReleasingHooks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
    #[inject]
    closes: Arc<Closes>,
}

#[hooks]
impl ReleasingHooks {
    #[on_application_shutdown]
    #[resilient(upstream, timeout = "100ms")]
    async fn release(&self) -> Refusable {
        self.dependency.call().await
    }

    #[on_application_shutdown]
    async fn close(&self) -> anyhow::Result<()> {
        self.closes.made.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[module(
    imports = [ResilienceModule::for_root(refusing())],
    providers = [ReleasingHooks, Dependency, Closes],
)]
struct RefusedLastShutdownModule;

/// The decorated hook `M` declares, `method` of `host` in the shutdown
/// `phase`, refused each way: its refusal is filed as any failure of a
/// shutdown hook, and the host's `close`, the last phase's hook after it,
/// still runs. `called` is that hook called as its own caller calls it.
async fn a_shutdown_hook_refused_is_filed_and_the_next_runs<M: Module + 'static>(
    phase: LifecyclePhase,
    (host, method): (&'static str, &'static str),
    called: impl AsyncFn(&Container) -> Refusable,
) {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<M>()
            .build_headless()
            .await
            .expect("a decorated hook boots");
        app.init().await.expect("no init hook fails");
        let closes = app.container().get::<Closes>().expect("provided");
        let container = app.container().clone();
        let refused = refusing.arrange(app.container()).await;
        let (upstream, dependency) = (&refused.upstream, &refused.dependency);
        let mut events = upstream.events();

        app.close().await;
        let told = crate::told(&mut events);
        assert_eq!(dependency.called(), refusing.sent());

        let failed = logs.expect_one(nest_rs_core::target::LIFECYCLE, "lifecycle hook failed");
        assert_eq!(failed.level, "error", "{failed:?}");
        assert_eq!(logs.events_at(tracing::Level::ERROR).len(), 1);
        let mut ran = logs.find(OPERATIONS, nest_rs_core::unit::HOOK.name());
        ran.retain(|line| line.field("host").as_deref() == Some(host));
        let [refused_hook, closed] = &ran[..] else {
            panic!("the refused hook, then the one after it: {ran:#?}");
        };
        assert_eq!(refused_hook.field("outcome").as_deref(), Some(ERROR));
        for line in [&failed, refused_hook] {
            assert_eq!(
                line.field("phase").as_deref(),
                Some(phase.as_str()),
                "{line:?}"
            );
            assert_eq!(line.field("host").as_deref(), Some(host));
            assert_eq!(line.field("method").as_deref(), Some(method));
            assert_eq!(line.field("origin").as_deref(), Some(module_path!()));
        }
        assert_eq!(
            closed.field("phase").as_deref(),
            Some(LifecyclePhase::OnApplicationShutdown.as_str())
        );
        assert_eq!(closed.field("method").as_deref(), Some("close"));
        assert_eq!(closed.field("outcome").as_deref(), Some(OK));
        assert_eq!(closes.made.load(Ordering::Relaxed), 1);
        refusing.told(&told, host, method);

        let chain = refusing.chain(called(&container).await);
        assert_eq!(
            failed.field("error").as_deref(),
            Some(&*chain),
            "{failed:?}"
        );
    }
}

/// No client reads a shutdown hook's `503`, in the first shutdown phase.
#[tokio::test(start_paused = true)]
async fn a_shutdown_hook_its_policy_refuses_is_filed_and_the_hook_after_it_runs() {
    a_shutdown_hook_refused_is_filed_and_the_next_runs::<RefusedShutdownModule>(
        LifecyclePhase::OnModuleDestroy,
        ("FlushingHooks", "flush"),
        async |container| hooks_of::<FlushingHooks>(container).flush().await,
    )
    .await;
}

/// Nor in the second, once every module tore down.
#[tokio::test(start_paused = true)]
async fn a_before_shutdown_hook_its_policy_refuses_is_filed_and_the_hook_after_it_runs() {
    a_shutdown_hook_refused_is_filed_and_the_next_runs::<RefusedBeforeShutdownModule>(
        LifecyclePhase::BeforeApplicationShutdown,
        ("DrainingHooks", "drain"),
        async |container| hooks_of::<DrainingHooks>(container).drain().await,
    )
    .await;
}

/// Nor in the last, where the hook after it is its host's next one.
#[tokio::test(start_paused = true)]
async fn a_last_shutdown_hook_its_policy_refuses_is_filed_and_the_hook_after_it_runs() {
    a_shutdown_hook_refused_is_filed_and_the_next_runs::<RefusedLastShutdownModule>(
        LifecyclePhase::OnApplicationShutdown,
        ("ReleasingHooks", "release"),
        async |container| hooks_of::<ReleasingHooks>(container).release().await,
    )
    .await;
}
