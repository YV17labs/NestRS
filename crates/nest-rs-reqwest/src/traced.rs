//! One request sent through a reqwest client, carrying the unit of work's trace
//! and recorded as the client operation the HTTP conventions name — the one
//! sequence [`ReqwestMiddleware`](crate::ReqwestMiddleware) and every reqwest
//! client the framework opens go through.

use std::future::Future;

use nest_rs_core::client_operation::HttpClientRequest;
use reqwest::{Request, Response};

/// How many times a middleware outside the traced send sent this request
/// before — the conventions' `http.request.resend_count` — handed in the
/// request's extensions by the resilience policy retrying it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resent(pub u32);

/// `request`, sent through `send` carrying the unit's trace, and recorded once
/// it answered, ran out of time as `timed_out` reads its error, or failed — as
/// the `resent`th sending again of a request that failed, `0` for a first one.
pub async fn traced<E, Sent>(
    mut request: Request,
    resent: Resent,
    send: impl FnOnce(Request) -> Sent,
    timed_out: impl FnOnce(&E) -> bool,
) -> Result<Response, E>
where
    Sent: Future<Output = Result<Response, E>>,
{
    let url = request.url();
    let sent = HttpClientRequest::start(request.method(), url.scheme(), url.host_str(), url.port())
        .resent(resent.0);
    sent.announce(request.headers_mut());
    let answer = send(request).await;
    match &answer {
        Ok(response) => sent.answered(response.status(), response.version()),
        Err(error) if timed_out(error) => sent.timed_out(),
        Err(_) => sent.failed(),
    }
    answer
}
