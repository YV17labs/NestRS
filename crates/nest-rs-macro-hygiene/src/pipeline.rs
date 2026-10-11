//! `#[interceptor]` at both stages: a consumer with no direct `async-trait`
//! or `tracing` dependency compiles the provider and the registration it
//! emits.

use std::sync::Arc;

use nest_rs::core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait, interceptor};

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

#[nest_rs::core::module(providers = [HygieneClock, HygieneAudit, HygieneTiming, HygieneCache])]
pub struct HygienePipelineModule;
