//! `#[interceptor]` at both stages, and an exception filter: a consumer with no
//! direct `async-trait` or `tracing` dependency compiles the provider and the
//! registration it emits — and the layers a route and the worker witnesses
//! bind.

use std::sync::Arc;

use nest_rs::core::{
    Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, UnitContext, async_trait,
    interceptor,
};

#[nest_rs::core::injectable]
#[derive(Default)]
pub struct HygieneClock;

#[interceptor]
pub struct HygieneAudit {
    #[inject]
    clock: Arc<HygieneClock>,
}

impl Layer for HygieneAudit {}

#[async_trait]
impl Interceptor for HygieneAudit {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let _ = &self.clock;
        next.run(cx).await
    }
}

#[interceptor(stage = unit)]
#[derive(Default)]
pub struct HygieneTiming;

impl Layer for HygieneTiming {}

#[async_trait]
impl Interceptor for HygieneTiming {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[interceptor(stage = handler)]
#[derive(Default)]
pub struct HygieneCache;

impl Layer for HygieneCache {}

#[async_trait]
impl Interceptor for HygieneCache {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

/// An interceptor a job, a tick and a listener bind on their block and their
/// method.
#[nest_rs::core::injectable]
#[derive(Default)]
pub struct HygieneUnitAudit;

impl Layer for HygieneUnitAudit {}

#[async_trait]
impl Interceptor for HygieneUnitAudit {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

/// Declines every failure: the exception filter a route, a job, a tick and a
/// listener bind.
#[nest_rs::core::injectable]
#[derive(Default)]
pub struct HygieneUnitRefusal;

impl Layer for HygieneUnitRefusal {}

#[async_trait]
impl ExceptionFilter for HygieneUnitRefusal {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Declined
    }
}

#[nest_rs::core::module(
    providers = [HygieneClock, HygieneAudit, HygieneTiming, HygieneCache, HygieneUnitRefusal]
)]
pub struct HygienePipelineModule;
