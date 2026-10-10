use std::time::Instant;

use async_trait::async_trait;
use nest_rs_core::{ContainerBuilder, Discoverable, Layer, MissingDependencyError};
use nest_rs_http::__private::{HttpEndpointWrap, endpoint_wrap_priority};
use nest_rs_interceptors::{Interceptor, InterceptorExt, Next};
use poem::http::{HeaderName, StatusCode};
use poem::{EndpointExt, Request, Response, Result};

use crate::config::ServerTimingConfig;
use crate::entry::Timings;
use crate::format::format_header;

const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

/// Answers that never carry a timing: the refusal of a credential, of a proxy's
/// credential and of a rate, where the time spent is an oracle (W3C Server
/// Timing §4; `.claude/decisions/server-timing-exposure.md`). Only the status
/// line is read: a refusal inside a `200`, a GraphQL or MCP error, keeps its
/// timing.
const UNTIMED: [StatusCode; 4] = [
    StatusCode::UNAUTHORIZED,
    StatusCode::FORBIDDEN,
    StatusCode::PROXY_AUTHENTICATION_REQUIRED,
    StatusCode::TOO_MANY_REQUESTS,
];

pub(crate) struct ServerTiming;

impl Layer for ServerTiming {}

#[async_trait]
impl Interceptor for ServerTiming {
    async fn intercept(&self, mut req: Request, next: Next<'_>) -> Result<Response> {
        let timings = Timings::default();
        req.extensions_mut().insert(timings.clone());
        let start = Instant::now();

        let result = next.run(req).await;
        let total = start.elapsed();

        let status = match &result {
            Ok(res) => res.status(),
            Err(err) => err.status(),
        };
        if UNTIMED.contains(&status) {
            return result;
        }
        let Some(value) = format_header(&timings.drain(), total) else {
            return result;
        };
        match result {
            Ok(mut res) => {
                // `append`: the handler may already have set a `Server-Timing`.
                res.headers_mut().append(SERVER_TIMING, value);
                Ok(res)
            }
            // An error response gets its timing too, keeping the error shape.
            Err(err) => {
                let mut res = err.into_response();
                res.headers_mut().append(SERVER_TIMING, value);
                Err(poem::Error::from_response(res))
            }
        }
    }
}

// Decided at register, from the config the factory phase resolved: a disabled
// header attaches no wrap, so the transport keeps its fused path.
impl Discoverable for ServerTiming {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let Some(config) = builder.snapshot().get::<ServerTimingConfig>() else {
            return builder.refuse(MissingDependencyError {
                module: "ServerTimingModule",
                consumer: "ServerTiming",
                dependency: "ServerTimingConfig",
            });
        };
        if !config.enabled {
            return builder;
        }
        builder.attach_meta::<Self, HttpEndpointWrap>(HttpEndpointWrap::with_priority(
            endpoint_wrap_priority::INTERCEPTORS,
            |_, endpoint| endpoint.interceptor(ServerTiming).boxed(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, module};

    use super::*;

    #[module(providers = [ServerTiming])]
    struct WithoutConfig;

    #[tokio::test]
    async fn a_header_with_no_config_refuses_the_boot_naming_it() {
        let Err(refusal) = App::builder().module::<WithoutConfig>().build().await else {
            panic!("a header that cannot read its switch must not boot");
        };
        let message = format!("{refusal:#}");
        assert!(
            message.contains("`ServerTiming`") && message.contains("`ServerTimingConfig`"),
            "the refusal names the provider and what it lacks: {message}",
        );
    }
}
