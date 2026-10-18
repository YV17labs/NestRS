//! Covers `src/interceptor.rs` through `#[interceptor]`: what a module brings
//! is the container's own provider, runs only where its module is imported,
//! after what the app lists and in import order, once whatever else lists it,
//! and the app's own list fails the boot on a type no module provides.

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nest_rs_core::__private::{
    GlobalLayers, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers, Tail,
    declare_handler, declare_site, declare_unit, dispatch, new_unit_context, settled,
};
use nest_rs_core::operation_log::Kind;
use nest_rs_core::{
    App, AppBuilder, Handler, Interceptor, Layer, Next, Outcome, Posture, UnitContext, UnitView,
    WiringFailedError, async_trait, injectable, interceptor, module,
};
use nest_rs_testing::LogCapture;

/// What ran, in order.
#[injectable]
#[derive(Default)]
struct Trail {
    steps: Mutex<Vec<&'static str>>,
}

impl Trail {
    fn push(&self, step: &'static str) {
        self.steps.lock().expect("the trail").push(step);
    }

    fn steps(&self) -> Vec<&'static str> {
        self.steps.lock().expect("the trail").clone()
    }
}

macro_rules! recording {
    ($name:ident, $step:literal $(, $stage:ident)?) => {
        #[interceptor$((stage = $stage))?]
        struct $name {
            #[inject]
            trail: Arc<Trail>,
        }

        impl Layer for $name {}

        #[async_trait]
        impl Interceptor for $name {
            async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
                self.trail.push($step);
                next.run(cx).await
            }
        }
    };
}

recording!(AppListed, "app");
recording!(Tracing, "tracing");
recording!(Metrics, "metrics");
recording!(Timing, "timing", unit);

#[module(providers = [Trail])]
struct TrailModule;

#[module(imports = [TrailModule], providers = [Tracing])]
struct TracingModule;

#[module(imports = [TrailModule], providers = [Metrics])]
struct MetricsModule;

#[module(imports = [TrailModule], providers = [Timing])]
struct TimingModule;

#[module(imports = [TrailModule], providers = [AppListed])]
struct AppListedModule;

struct Jobs;
struct JobView;
impl UnitView for JobView {}

static RUN: Handler = declare_handler(HandlerDeclaration {
    unit: declare_unit(
        "nest-rs-queue",
        "nest_rs::queue",
        Kind::Consumer,
        "queue.job",
    ),
    host: TypeId::of::<Jobs>(),
    host_name: "Jobs",
    method: "run",
    address: "jobs",
    posture: Posture::System,
    meta: MetaLevels::default,
});

static SITE: Site = declare_site(
    &RUN,
    SiteDeclaration {
        layers: SiteLayers::default,
        global: GlobalLayers::ALL,
        guard_stage: None,
        class_gate: None,
    },
);

struct Handled;

impl Tail for Handled {
    async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
        Ok(settled())
    }
}

/// Run one unit of [`SITE`] on `app`, and what ran.
async fn one_unit(app: &App) -> Vec<&'static str> {
    let chains = SITE
        .chains(app.container())
        .unwrap_or_else(|refused| panic!("the site composes: {refused}"));
    let mut view = JobView;
    let mut cx = new_unit_context(&RUN, &mut view, None, None);
    assert!(dispatch(&mut cx, &chains, &mut Handled).await.is_ok());
    app.container().get::<Trail>().expect("provided").steps()
}

async fn boot(builder: AppBuilder) -> App {
    builder
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the app boots: {refused:#}"))
}

#[module(imports = [AppListedModule, MetricsModule, TracingModule])]
struct ImportsMetricsFirst;

#[module(imports = [AppListedModule, TracingModule, MetricsModule])]
struct ImportsTracingFirst;

#[tokio::test]
async fn a_module_interceptor_runs_after_the_apps_own_in_import_order() {
    let metrics_first = App::builder()
        .module::<ImportsMetricsFirst>()
        .use_interceptors_global([nest_rs_core::interceptor::<AppListed>()]);
    assert_eq!(
        one_unit(&boot(metrics_first).await).await,
        ["app", "metrics", "tracing"],
        "the app's list first, then each module's in the order the root imports them",
    );

    let tracing_first = App::builder()
        .module::<ImportsTracingFirst>()
        .use_interceptors_global([nest_rs_core::interceptor::<AppListed>()]);
    assert_eq!(
        one_unit(&boot(tracing_first).await).await,
        ["app", "tracing", "metrics"]
    );
}

#[module(imports = [TimingModule])]
struct ImportsTiming;

#[tokio::test]
async fn an_interceptor_at_the_unit_stage_the_app_also_lists_runs_once() {
    let builder = App::builder()
        .module::<ImportsTiming>()
        .use_interceptors_global([nest_rs_core::interceptor::<Timing>()]);
    let app = boot(builder).await;
    assert_eq!(one_unit(&app).await, ["timing"]);
}

/// Counts the units it saw, on itself.
#[interceptor]
struct Counting {
    #[inject]
    trail: Arc<Trail>,
    seen: AtomicUsize,
}

impl Layer for Counting {}

#[async_trait]
impl Interceptor for Counting {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        self.seen.fetch_add(1, Ordering::SeqCst);
        self.trail.push("counting");
        next.run(cx).await
    }
}

#[module(providers = [Counting, Trail])]
struct CountingBeforeItsDependency;

#[tokio::test]
async fn the_interceptor_a_unit_runs_is_the_containers_own_provider() {
    let builder = App::builder()
        .module::<CountingBeforeItsDependency>()
        .use_interceptors_global([nest_rs_core::interceptor::<Counting>()]);
    let app = boot(builder).await;
    assert_eq!(one_unit(&app).await, ["counting"], "listed twice, run once");
    let counting = app.container().get::<Counting>().expect("provided");
    assert_eq!(
        counting.seen.load(Ordering::SeqCst),
        1,
        "the instance the container holds is the one the unit ran",
    );
}

#[module(imports = [TrailModule])]
struct ImportsNoInterceptor;

#[tokio::test]
async fn a_module_the_app_does_not_import_brings_no_interceptor() {
    let app = boot(App::builder().module::<ImportsNoInterceptor>()).await;
    assert!(
        one_unit(&app).await.is_empty(),
        "linked into the binary, `Tracing` and `Metrics` stay out of a unit"
    );
}

#[tokio::test]
async fn an_interceptor_the_app_lists_and_no_module_provides_fails_the_boot_naming_it() {
    let built = App::builder()
        .module::<ImportsNoInterceptor>()
        .use_interceptors_global([nest_rs_core::interceptor::<Metrics>()])
        .build()
        .await;
    let Err(refused) = built else {
        panic!("a global interceptor would silently drop");
    };
    assert!(
        refused.downcast_ref::<WiringFailedError>().is_some(),
        "not the wiring refusal: {refused:#}"
    );
    let said = format!("{refused:#}");
    assert!(said.contains("nest_rs::layers::interceptors"), "{said}");
    assert!(said.contains("Metrics"), "{said}");
}

#[tokio::test]
async fn the_boot_says_each_interceptors_stage() {
    let logs = LogCapture::install();
    boot(App::builder().module::<ImportsTiming>()).await;
    let registered = logs.expect_one(nest_rs_core::target::LAYERS, "interceptor registered");
    assert_eq!(registered.level, "debug");
    assert_eq!(registered.field("layer").as_deref(), Some("Timing"));
    assert_eq!(registered.field("stage").as_deref(), Some("unit"));
}
