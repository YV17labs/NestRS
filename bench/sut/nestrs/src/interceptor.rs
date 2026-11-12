use nest_rs::core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable};

#[injectable]
#[derive(Layer)]
pub struct PassInterceptor;

#[async_trait]
impl Interceptor for PassInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}
