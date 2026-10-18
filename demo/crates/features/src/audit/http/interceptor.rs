use nest_rs::core::operation_log::Unit;
use nest_rs::core::{
    Edge, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, interceptor,
};
use nest_rs::http::HttpView;

#[interceptor]
#[derive(Default)]
pub struct AuditInterceptor;

impl Layer for AuditInterceptor {
    fn serves(&self, unit: Unit) -> bool {
        unit.edge() == Some(Edge::Http)
    }
}

#[async_trait]
impl Interceptor for AuditInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        let handler = cx.handler();
        if let Ok(view) = HttpView::of(cx)
            && !view.request().method().is_safe()
        {
            tracing::info!(
                target: crate::audit::TARGET,
                host = handler.host_name(),
                handler = handler.method(),
                status = view.response().map(|resp| resp.status().as_u16()),
                "write audited",
            );
        }
        outcome
    }
}
