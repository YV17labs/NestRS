//! Covers `src/exception_filter.rs` through a booted app: a filter the app's
//! root declares is the container's own provider and answers a failed unit at
//! any site.

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::__private::{
    GlobalLayers, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers, Tail,
    declare_handler, declare_site, declare_unit, dispatch, new_unit_context, settled,
};
use nest_rs_core::operation_log::Kind;
use nest_rs_core::{
    App, Caught, ExceptionFilter, GlobalExceptionFilters, Handler, Layer, Outcome, Posture,
    UnitContext, UnitView, async_trait, injectable, module,
};

/// Answers every failure, counting the ones it answered.
#[injectable]
#[derive(Default, Layer)]
struct CatchAll {
    answered: AtomicUsize,
}

#[async_trait]
impl ExceptionFilter for CatchAll {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        self.answered.fetch_add(1, Ordering::SeqCst);
        Caught::Answered(settled())
    }
}

#[module(providers = [GlobalExceptionFilters<(CatchAll,)>])]
struct CatchAllModule;

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

struct Failing;

impl Tail for Failing {
    async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
        Err(std::io::Error::other("the job failed").into())
    }
}

#[tokio::test]
async fn a_filter_the_root_declares_answers_a_failed_unit_with_the_containers_own_provider() {
    let app = App::builder()
        .module::<CatchAllModule>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the app boots: {refused:#}"));
    let chains = SITE
        .chains(app.container())
        .unwrap_or_else(|refused| panic!("the site composes: {refused}"));

    let mut view = JobView;
    let mut cx = new_unit_context(&RUN, &mut view, None, None);
    let failure = dispatch(&mut cx, &chains, &mut Failing)
        .await
        .expect_err("an answered unit stays failed");
    assert!(failure.is_answered(), "the app's filter answered it");
    let filter = app.container().get::<CatchAll>().expect("provided");
    assert_eq!(
        filter.answered.load(Ordering::SeqCst),
        1,
        "the instance the container holds is the one that answered",
    );
}
