//! A pool registers what it lists, so each entry is a provider: a type no
//! decorator made one is refused where the pool names it, not at the boot.

use nest_rs::core::{
    GlobalInterceptors, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, module,
};

#[derive(Layer)]
struct Audit;

#[async_trait]
impl Interceptor for Audit {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[module(providers = [GlobalInterceptors<(Audit,)>])]
struct AppModule;

fn main() {}
