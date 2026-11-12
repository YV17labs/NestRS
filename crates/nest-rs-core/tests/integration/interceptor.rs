//! Covers `src/interceptor.rs` through `#[interceptor(stage = unit)]` and the
//! root's pool: what a module brings is the container's own provider, runs only
//! where its module is imported, around what the app's root declares and in
//! import order — a `#[derive(Layer)]` priority ahead of the others — and once
//! whatever else lists it.

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nest_rs_core::__private::{
    GlobalLayers, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers, Tail,
    declare_handler, declare_site, declare_unit, dispatch, new_unit_context, settled,
};
use nest_rs_core::operation_log::Kind;
use nest_rs_core::{
    App, AppBuilder, GlobalInterceptors, Handler, Interceptor, Layer, Next, Outcome, Posture,
    UnitContext, UnitView, async_trait, injectable, interceptor, module,
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
    ($name:ident, $step:literal, $declared:meta) => {
        #[$declared]
        #[derive(Layer)]
        struct $name {
            #[inject]
            trail: Arc<Trail>,
        }

        #[async_trait]
        impl Interceptor for $name {
            async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
                self.trail.push($step);
                next.run(cx).await
            }
        }
    };
}

recording!(AppListed, "app", injectable);
recording!(Tracing, "tracing", interceptor(stage = unit));
recording!(Metrics, "metrics", interceptor(stage = unit));
recording!(Timing, "timing", interceptor(stage = unit));

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

#[module(
    imports = [AppListedModule, MetricsModule, TracingModule],
    providers = [GlobalInterceptors<(AppListed,)>],
)]
struct ImportsMetricsFirst;

#[module(
    imports = [AppListedModule, TracingModule, MetricsModule],
    providers = [GlobalInterceptors<(AppListed,)>],
)]
struct ImportsTracingFirst;

#[tokio::test]
async fn a_module_interceptor_runs_around_the_apps_own_in_import_order() {
    let metrics_first = App::builder().module::<ImportsMetricsFirst>();
    assert_eq!(
        one_unit(&boot(metrics_first).await).await,
        ["metrics", "tracing", "app"],
        "each module's in the order the root imports them, around the app's list",
    );

    let tracing_first = App::builder().module::<ImportsTracingFirst>();
    assert_eq!(
        one_unit(&boot(tracing_first).await).await,
        ["tracing", "metrics", "app"]
    );
}

/// Brought by its module, ordered ahead of the stage's others by the priority
/// its derive declares.
#[interceptor(stage = unit)]
#[derive(Layer)]
#[layer(priority = -1)]
struct Prioritized {
    #[inject]
    trail: Arc<Trail>,
}

#[async_trait]
impl Interceptor for Prioritized {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        self.trail.push("prioritized");
        next.run(cx).await
    }
}

#[module(imports = [TrailModule], providers = [Prioritized])]
struct PrioritizedModule;

#[module(
    imports = [AppListedModule, TracingModule, PrioritizedModule],
    providers = [GlobalInterceptors<(AppListed,)>],
)]
struct ImportsPrioritizedLast;

#[tokio::test]
async fn a_derived_priority_orders_an_interceptor_ahead_of_its_stage() {
    let builder = App::builder().module::<ImportsPrioritizedLast>();
    assert_eq!(
        one_unit(&boot(builder).await).await,
        ["prioritized", "tracing", "app"],
        "`#[layer(priority = -1)]` runs first, import order notwithstanding",
    );
}

#[module(imports = [TimingModule], providers = [GlobalInterceptors<(Timing,)>])]
struct ImportsTiming;

#[tokio::test]
async fn an_interceptor_at_the_unit_stage_the_app_also_lists_runs_once() {
    let builder = App::builder().module::<ImportsTiming>();
    let app = boot(builder).await;
    assert_eq!(one_unit(&app).await, ["timing"]);
}

/// Counts the units it saw, on itself.
#[injectable]
#[derive(Layer)]
struct Counting {
    #[inject]
    trail: Arc<Trail>,
    seen: AtomicUsize,
}

#[async_trait]
impl Interceptor for Counting {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        self.seen.fetch_add(1, Ordering::SeqCst);
        self.trail.push("counting");
        next.run(cx).await
    }
}

#[module(providers = [Counting, Trail, GlobalInterceptors<(Counting,)>])]
struct CountingBeforeItsDependency;

#[tokio::test]
async fn the_interceptor_a_unit_runs_is_the_containers_own_provider() {
    let builder = App::builder().module::<CountingBeforeItsDependency>();
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
async fn the_boot_says_what_a_module_brings() {
    let logs = LogCapture::install();
    boot(App::builder().module::<ImportsTiming>()).await;
    let registered = logs.expect_one(nest_rs_core::target::LAYERS, "interceptor registered");
    assert_eq!(registered.level, "debug");
    assert_eq!(registered.field("layer").as_deref(), Some("Timing"));
}
