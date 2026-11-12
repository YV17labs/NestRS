//! An app-wide interceptor at the handler stage is the root's: `#[interceptor]`
//! takes `stage = unit` alone, and the refusal names where the other goes, bare
//! or spelled.

use nest_rs::core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait, interceptor};

#[interceptor]
#[derive(Default, Layer)]
struct Bare;

#[interceptor(stage = handler)]
#[derive(Default, Layer)]
struct Spelled;

#[async_trait]
impl Interceptor for Bare {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[async_trait]
impl Interceptor for Spelled {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

fn main() {}
