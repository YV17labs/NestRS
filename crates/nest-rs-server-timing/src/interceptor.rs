use std::time::Instant;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::HeaderName;
use poem::{Request, Response, Result};

use crate::entry::Timings;
use crate::format::format_header;

const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

#[interceptor]
#[derive(Default)]
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

        let header = format_header(&timings.drain(), total);
        match result {
            Ok(mut res) => {
                if let Some(value) = header {
                    // `append` (not `insert`): the spec allows multiple
                    // `Server-Timing` headers and a downstream interceptor or
                    // the handler may already have set one.
                    res.headers_mut().append(SERVER_TIMING, value);
                }
                Ok(res)
            }
            // An error response deserves its timing too — attach the header
            // to the rendered response and keep the error shape for outer
            // layers.
            Err(err) => {
                let Some(value) = header else { return Err(err) };
                let mut res = err.into_response();
                res.headers_mut().append(SERVER_TIMING, value);
                Err(poem::Error::from_response(res))
            }
        }
    }
}
