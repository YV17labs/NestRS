//! What one unit pays the pipeline, measured without an edge: a dispatch of
//! each shape — `Bare`, `GateOnly`, `Layered` with 0, 1 and 3 interceptors —
//! the ambient read and install, a task's capture, a chain's composition and
//! the lookup of a site's chains a container already composed.
//!
//! `cargo bench -p nest-rs-core --bench pipeline`; the protocol and how a run
//! is reported are `bench/RUNBOOK.md`'s.
#![allow(
    clippy::expect_used,
    reason = "a benchmark that cannot set itself up has nothing to measure; it stops there"
)]

use std::any::TypeId;
use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nest_rs_core::__private::{
    ClassGate, GlobalLayers, GuardStage, GuardStageFactory, HandlerDeclaration, MetaLevels,
    ResolvedLayer, Site, SiteDeclaration, SiteLayers, Tail, UnitChains, compose_chain,
    declare_handler, declare_site, declare_unit, dispatch, new_unit_context, settled,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::operation_log::Kind;
use nest_rs_core::{
    AmbientValue, Caught, Container, Continuation, Correlation, ExceptionFilter, Failure, Handler,
    Interceptor, Layer, Next, Outcome, Posture, RequestContinuation, TaskContext, UnitContext,
    UnitView, ambient, async_trait, exception_filter, interceptor, with_ambient,
};

struct Bench;

/// The one handler every site here dispatches to.
static HANDLER: Handler = declare_handler(HandlerDeclaration {
    unit: declare_unit(
        "nest-rs-http",
        "nest_rs::http",
        Kind::Server,
        "http.request",
    ),
    host: TypeId::of::<Bench>(),
    host_name: "Bench",
    method: "answer",
    address: "/bench",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

/// The view the edge would hand its layers; nothing here reads it.
struct View;

impl UnitView for View {}

/// The handler: answers at once.
struct Answer;

impl Tail for Answer {
    async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
        Ok(settled())
    }
}

/// A guard stage admitting every unit, as a passing guard does.
struct Admit;

impl GuardStage for Admit {
    fn check<'a>(
        &'a self,
        _: &'a mut UnitContext<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Failure>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

fn admitting(
    _: &Container,
    _: &Handler,
) -> nest_rs_core::anyhow::Result<Option<Arc<dyn GuardStage>>> {
    Ok(Some(Arc::new(Admit)))
}

/// A class gate admitting every unit.
fn gate(_: &mut UnitContext<'_>) -> Result<(), Failure> {
    Ok(())
}

/// An interceptor that runs the rest of the unit and returns its outcome.
#[derive(Default)]
struct Pass<const N: usize>;

impl<const N: usize> Layer for Pass<N> {}

#[async_trait]
impl<const N: usize> Interceptor for Pass<N> {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

/// A filter that would decline every failure; a unit that succeeds never runs it.
#[derive(Default)]
struct Decline;

impl Layer for Decline {}

#[async_trait]
impl ExceptionFilter for Decline {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Declined
    }
}

fn a_filter() -> SiteLayers {
    SiteLayers {
        method_exception_filters: vec![exception_filter::<Decline>()],
        ..SiteLayers::default()
    }
}

fn one_interceptor() -> SiteLayers {
    SiteLayers {
        method_exception_filters: vec![exception_filter::<Decline>()],
        method_interceptors: vec![interceptor::<Pass<0>>()],
        ..SiteLayers::default()
    }
}

fn three_interceptors() -> SiteLayers {
    SiteLayers {
        method_exception_filters: vec![exception_filter::<Decline>()],
        host_interceptors: vec![interceptor::<Pass<0>>()],
        method_interceptors: vec![interceptor::<Pass<1>>(), interceptor::<Pass<2>>()],
        ..SiteLayers::default()
    }
}

const fn site(
    layers: fn() -> SiteLayers,
    guard_stage: Option<GuardStageFactory>,
    class_gate: Option<ClassGate>,
) -> Site {
    declare_site(
        &HANDLER,
        SiteDeclaration {
            layers,
            global: GlobalLayers::ALL,
            guard_stage,
            class_gate,
        },
    )
}

static BARE: Site = site(SiteLayers::default, None, None);
static GATE_ONLY: Site = site(SiteLayers::default, Some(admitting), Some(gate));
static LAYERED_0: Site = site(a_filter, Some(admitting), Some(gate));
static LAYERED_1: Site = site(one_interceptor, Some(admitting), Some(gate));
static LAYERED_3: Site = site(three_interceptors, Some(admitting), Some(gate));

fn container() -> Container {
    Container::builder()
        .provide(Pass::<0>)
        .provide(Pass::<1>)
        .provide(Pass::<2>)
        .provide(Decline)
        .build()
        .expect("the bench's container builds")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
}

/// The context an edge installs around a unit: the request's identity and
/// scope, which every ambient read and install below runs inside.
fn request() -> RequestContinuation {
    RequestContinuation::new(None, Correlation::minted(None))
}

/// `iters` units of `run`, timed inside one request context on `rt`.
fn timed<F, Fut>(rt: &tokio::runtime::Runtime, iters: u64, run: F) -> Duration
where
    F: Fn() -> Fut,
    Fut: Future,
{
    rt.block_on(request().scope(async {
        let started = Instant::now();
        for _ in 0..iters {
            black_box(run().await);
        }
        started.elapsed()
    }))
}

async fn one_unit(chains: &UnitChains) -> bool {
    let mut view = View;
    let mut cx = new_unit_context(&HANDLER, &mut view, None, None);
    dispatch(&mut cx, chains, &mut Answer).await.is_ok()
}

fn dispatch_by_shape(c: &mut Criterion) {
    let rt = runtime();
    let container = container();
    let mut group = c.benchmark_group("dispatch");
    for (name, site) in [
        ("bare", &BARE),
        ("gate_only", &GATE_ONLY),
        ("layered_0", &LAYERED_0),
        ("layered_1", &LAYERED_1),
        ("layered_3", &LAYERED_3),
    ] {
        let chains = site.chains(&container).expect("the site composes");
        group.bench_function(name, |b| {
            b.iter_custom(|iters| timed(&rt, iters, || one_unit(&chains)));
        });
    }
    group.finish();
}

/// What a guard attaches and a service reads back.
struct Principal(#[expect(dead_code, reason = "a value of some size, read by nothing")] u64);

impl AmbientValue for Principal {}

fn ambient_values(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("ambient");
    group.bench_function("read", |b| {
        b.iter_custom(|iters| {
            rt.block_on(
                request().scope(with_ambient(Arc::new(Principal(7)), async move {
                    let started = Instant::now();
                    for _ in 0..iters {
                        black_box(ambient::<Principal>());
                    }
                    started.elapsed()
                })),
            )
        });
    });
    let principal = Arc::new(Principal(7));
    group.bench_function("with_ambient", |b| {
        b.iter_custom(|iters| {
            timed(&rt, iters, || {
                with_ambient(Arc::clone(&principal), async { black_box(1_u8) })
            })
        });
    });
    group.bench_function("task_capture", |b| {
        b.iter_custom(|iters| {
            rt.block_on(
                request().scope(with_ambient(Arc::new(Principal(7)), async move {
                    let started = Instant::now();
                    for _ in 0..iters {
                        black_box(TaskContext::capture(Continuation::Task));
                    }
                    started.elapsed()
                })),
            )
        });
    });
    group.finish();
}

fn resolved<const N: usize>(source: LayerSite) -> ResolvedLayer<dyn Interceptor> {
    ResolvedLayer {
        type_id: TypeId::of::<Pass<N>>(),
        name: "Pass",
        source,
        layer: Arc::new(Pass::<N>),
    }
}

fn composition(c: &mut Criterion) {
    let mut group = c.benchmark_group("compose");
    for overlap in [false, true] {
        group.bench_with_input(
            BenchmarkId::new(
                "compose_chain",
                if overlap { "deduped" } else { "distinct" },
            ),
            &overlap,
            |b, &overlap| {
                b.iter(|| {
                    let global = vec![resolved::<0>(LayerSite::Global)];
                    let host = vec![resolved::<1>(LayerSite::Host)];
                    let method = match overlap {
                        true => vec![
                            resolved::<0>(LayerSite::Method),
                            resolved::<1>(LayerSite::Method),
                        ],
                        false => vec![resolved::<2>(LayerSite::Method)],
                    };
                    black_box(compose_chain(global, host, method, &[], "Bench::answer"))
                });
            },
        );
    }
    let container = container();
    LAYERED_3
        .chains(&container)
        .expect("the site composes once");
    group.bench_function("site_lookup", |b| {
        b.iter(|| black_box(LAYERED_3.chains(&container).expect("the composed chains")));
    });
    group.finish();
}

criterion_group!(benches, dispatch_by_shape, ambient_values, composition);
criterion_main!(benches);
