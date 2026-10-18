//! One of the four refusals a `key = value` grammar owes, pinned where the
//! compiler says it. Refusals are shared, not per key. One
//! helper, one sentence, every key it covers, **one trybuild snapshot per
//! site**.
//!
//! The second is a misspelling followed by another key: it must be named
//! itself, never answered as a repeat of a key never written.

use nest_rs::core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait, interceptor};

#[interceptor(order = 100)]
#[derive(Default)]
struct Tracing;

#[interceptor(stag = unit, stage = handler)]
#[derive(Default)]
struct Timing;

impl Layer for Tracing {}
impl Layer for Timing {}

#[async_trait]
impl Interceptor for Tracing {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[async_trait]
impl Interceptor for Timing {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

fn main() {}
