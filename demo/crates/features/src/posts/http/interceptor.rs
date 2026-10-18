use nest_rs::core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable};
use nest_rs::http::HttpView;

#[injectable]
#[derive(Default)]
pub struct PostAuditInterceptor;

impl Layer for PostAuditInterceptor {}

#[async_trait]
impl Interceptor for PostAuditInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        let status = HttpView::of(cx)
            .ok()
            .and_then(|view| view.response())
            .map(|resp| resp.status().as_u16());
        tracing::info!(
            target: crate::posts::TARGET,
            status,
            "post request audited",
        );
        outcome
    }
}
