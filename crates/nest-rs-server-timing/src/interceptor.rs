use std::time::Instant;

use nest_rs_config::ConfigModule;
use nest_rs_core::__private::{InterceptorStage, register_interceptor};
use nest_rs_core::operation_log::Unit;
use nest_rs_core::{
    ContainerBuilder, Discoverable, Edge, Interceptor, Layer, Next, Outcome, UnitContext,
    async_trait,
};
use nest_rs_guards::__private::RefusalMark;
use nest_rs_http::{HeaderName, HttpView, StatusCode};

use crate::config::ServerTimingConfig;
use crate::entry::Timings;
use crate::format::format_header;

const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

/// Answers that never carry a timing: the refusal of a credential, of a proxy's
/// credential and of a rate, where the time spent is an oracle (W3C Server
/// Timing §4; `.claude/decisions/server-timing-exposure.md`). A refusal a unit
/// inside the request answered within a `200` — a GraphQL or an MCP operation
/// its guards refused — marks the request ([`RefusalMark`]) and goes untimed
/// too.
const UNTIMED: [StatusCode; 4] = [
    StatusCode::UNAUTHORIZED,
    StatusCode::FORBIDDEN,
    StatusCode::PROXY_AUTHENTICATION_REQUIRED,
    StatusCode::TOO_MANY_REQUESTS,
];

/// Whether `response` is an event stream, whose head leaves before the work it
/// streams — an MCP operation its guards may yet refuse — so a timing there
/// measures nothing and could time a refusal.
fn streams_events(response: &nest_rs_http::Response) -> bool {
    response
        .headers()
        .get(nest_rs_http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_start().starts_with("text/event-stream"))
}

/// The unit-stage interceptor timing every HTTP unit — a route, a self-mount's
/// request, what the router answers itself — its refusals aside.
pub(crate) struct ServerTiming;

impl Layer for ServerTiming {
    fn serves(&self, unit: Unit) -> bool {
        unit.edge() == Some(Edge::Http)
    }
}

#[async_trait]
impl Interceptor for ServerTiming {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let timings = Timings::default();
        HttpView::of(cx)?
            .request_mut()
            .extensions_mut()
            .insert(timings.clone());
        let refused = RefusalMark::default();
        cx.attach(refused.clone());
        let start = Instant::now();

        let outcome = next.run(cx).await;
        let total = start.elapsed();

        if refused.is_marked() {
            return outcome;
        }
        let Some(response) = HttpView::of(cx)?.response_mut() else {
            return outcome;
        };
        if UNTIMED.contains(&response.status()) || streams_events(response) {
            return outcome;
        }
        if let Some(value) = format_header(&timings.drain(), total) {
            // `append`: the handler may already have set a `Server-Timing`.
            response.headers_mut().append(SERVER_TIMING, value);
        }
        outcome
    }
}

// Decided at register, from the config the factory phase resolved: a disabled
// header registers nothing, so no unit runs a stage for it.
impl Discoverable for ServerTiming {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let config = match ConfigModule::resolved::<ServerTimingConfig>(&builder.snapshot()) {
            Ok(config) => config,
            Err(refusal) => return builder.refuse(refusal),
        };
        if !config.enabled {
            return builder;
        }
        register_interceptor::<Self>(builder.provide(ServerTiming), InterceptorStage::Unit)
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::ConfigNotResolvedError;
    use nest_rs_core::{App, module};

    use super::*;

    #[module(providers = [ServerTiming])]
    struct WithoutConfig;

    #[tokio::test]
    async fn a_header_with_no_config_refuses_the_boot_naming_it() {
        let Err(refusal) = App::builder().module::<WithoutConfig>().build().await else {
            panic!("a header that cannot read its switch must not boot");
        };
        let unresolved = refusal
            .downcast_ref::<ConfigNotResolvedError>()
            .unwrap_or_else(|| panic!("not the config refusal: {refusal:#}"));
        assert_eq!(
            unresolved.type_name,
            std::any::type_name::<ServerTimingConfig>()
        );
    }
}
