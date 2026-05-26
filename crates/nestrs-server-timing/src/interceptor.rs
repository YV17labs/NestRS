use std::time::Instant;

use async_trait::async_trait;
use nestrs_http::interceptor;
use nestrs_middleware::{Interceptor, Next};
use poem::http::HeaderName;
use poem::{Request, Response, Result};

use crate::entry::Timings;
use crate::format::format_header;

const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

#[interceptor]
#[derive(Default)]
pub(crate) struct ServerTiming;

#[async_trait]
impl Interceptor for ServerTiming {
    async fn intercept(&self, mut req: Request, next: Next<'_>) -> Result<Response> {
        let timings = Timings::default();
        req.extensions_mut().insert(timings.clone());
        let start = Instant::now();

        let mut res = next.run(req).await?;
        let total = start.elapsed();

        if let Some(value) = format_header(&timings.drain(), total) {
            // `append` (not `insert`): the spec explicitly allows multiple
            // `Server-Timing` headers in one response and requires the UA to
            // process all of them. Insert would clobber a header set by the
            // handler or a downstream interceptor.
            res.headers_mut().append(SERVER_TIMING, value);
        }
        Ok(res)
    }
}
