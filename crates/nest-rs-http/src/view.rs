//! [`HttpView`], the HTTP edge's view of the request it dispatches.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

use nest_rs_core::__private::new_unit_context;
use nest_rs_core::{Handler, Settled, UnitContext, UnitView};
use nest_rs_guards::Denial;

use crate::request::Request;
use crate::response::Response;

/// The HTTP edge's view of one request: what a guard reads through
/// [`HttpView::of`] — the request, head and body — the route it matched, and,
/// once the unit answered, the response a layer around it reads or rewrites.
///
/// A guard that reads the body sets it back for the handler:
///
/// ```
/// use nest_rs_guards::prelude::*;
/// use nest_rs_http::HttpView;
///
/// struct SignedBody;
///
/// impl Layer for SignedBody {}
///
/// #[async_trait]
/// impl Guard for SignedBody {
///     async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
///         let request = HttpView::of(cx)?.request_mut();
///         let body = request
///             .take_body()
///             .into_bytes()
///             .await
///             .map_err(|_| Denial::unauthorized("an unreadable body"))?;
///         let signed = request.header("x-signature").is_some() && !body.is_empty();
///         request.set_body(body);
///         if signed { Ok(()) } else { Err(Denial::unauthorized("an unsigned body")) }
///     }
/// }
///
/// impl GuardFor<HttpView> for SignedBody {}
/// ```
///
/// An interceptor reads the answer once the rest of the unit ran, or answers
/// instead of it:
///
/// ```
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
/// use nest_rs_http::{HeaderValue, HttpView, Response, StatusCode};
///
/// struct Maintenance;
///
/// impl Layer for Maintenance {}
///
/// #[async_trait]
/// impl Interceptor for Maintenance {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         let http = HttpView::of(cx)?;
///         if http.request().header("x-maintenance").is_some() {
///             let closed = Response::builder()
///                 .status(StatusCode::SERVICE_UNAVAILABLE)
///                 .finish();
///             return Ok(http.respond(closed));
///         }
///         let outcome = next.run(cx).await;
///         if let Some(response) = HttpView::of(cx)?.response_mut() {
///             response
///                 .headers_mut()
///                 .insert("x-served-by", HeaderValue::from_static("api"));
///         }
///         outcome
///     }
/// }
/// ```
pub struct HttpView {
    request: Request,
    response: Option<Response>,
}

impl UnitView for HttpView {}

impl HttpView {
    /// The view of `request`, as the edge builds it for a unit.
    pub(crate) fn new(request: Request) -> Self {
        Self {
            request,
            response: None,
        }
    }

    /// The request back, once the stage that read it is over.
    pub(crate) fn into_request(self) -> Request {
        self.request
    }

    /// The response the unit answered, taken by the edge that writes it.
    pub(crate) fn take_response(&mut self) -> Option<Response> {
        self.response.take()
    }

    /// The HTTP view of `cx`, or an opaque [`Denial::internal`] when an HTTP
    /// entry ran on a unit with no HTTP view — a framework wiring defect, never
    /// a client's doing, logged at `error` once per site.
    pub fn of<'c>(cx: &'c mut UnitContext<'_>) -> Result<&'c mut HttpView, Denial> {
        if cx.switch_to::<HttpView>().is_none() {
            report_viewless(cx);
        }
        cx.switch_to_mut::<HttpView>().ok_or_else(|| {
            Denial::internal("an HTTP check ran on a unit the HTTP edge did not dispatch")
        })
    }

    /// The request: its head, its extensions and what the connection knows of
    /// it, for the whole unit — an exception filter, or an interceptor once the
    /// rest of the unit ran, reads them as a guard does — and its body until a
    /// reader takes it, the handler at the latest.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The request, to read its body or set it back.
    pub fn request_mut(&mut self) -> &mut Request {
        &mut self.request
    }

    /// The template the router matched, as registered (`/users/{id}`), or
    /// `None` on a request no route matched.
    pub fn route(&self) -> Option<&str> {
        self.request.route()
    }

    /// The response the unit answered, once the rest of it ran or a layer
    /// answered through [`respond`](Self::respond).
    pub fn response(&self) -> Option<&Response> {
        self.response.as_ref()
    }

    /// The response the unit answered, to rewrite a header or its body.
    pub fn response_mut(&mut self) -> Option<&mut Response> {
        self.response.as_mut()
    }

    /// Answer the unit with `response`, in place of whatever it held: the proof
    /// an interceptor that answers instead of running the rest returns.
    pub fn respond(&mut self, response: Response) -> Settled {
        self.response = Some(response);
        nest_rs_core::__private::settled()
    }
}

/// `f`'s answer over a unit context on `request`'s view, as the unit `handler`
/// declares — what a fold an endpoint runs on its request, a global pipe,
/// reads. The request is the view's while `f` runs, then back in place.
pub fn with_view<R>(
    handler: &'static Handler,
    request: &mut Request,
    f: impl FnOnce(&UnitContext<'_>) -> R,
) -> R {
    let mut view = HttpView::new(std::mem::replace(request, Request::builder().finish()));
    let answer = f(&new_unit_context(handler, &mut view, None, None));
    *request = view.into_request();
    answer
}

/// Say once per site that an HTTP entry ran on a unit with no HTTP view.
fn report_viewless(cx: &UnitContext<'_>) {
    static REPORTED: LazyLock<Mutex<HashSet<usize>>> = LazyLock::new(Default::default);
    let site = std::ptr::from_ref(cx.handler()) as usize;
    // A poisoned set repeats the line, which beats a defect said nowhere.
    let first = REPORTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(site);
    if first {
        tracing::error!(
            target: crate::target::HTTP,
            unit = cx.unit().name(),
            host = cx.handler().host_name(),
            method = cx.handler().method(),
            "an HTTP guard entry ran on a unit with no HTTP view",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::__private::{
        HandlerDeclaration, MetaLevels, declare_handler, new_unit_context,
    };
    use nest_rs_core::{Handler, Posture};

    use super::*;

    struct Routes;

    static ROUTE: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::REQUEST,
        host: TypeId::of::<Routes>(),
        host_name: "Routes",
        method: "route",
        address: "/x",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    struct Socket;

    impl UnitView for Socket {}

    #[test]
    fn a_unit_the_http_edge_dispatched_reads_its_request() {
        let mut view = HttpView::new(Request::builder().uri("/x".parse().unwrap()).finish());
        let mut cx = new_unit_context(&ROUTE, &mut view, None, None);
        let request = HttpView::of(&mut cx).expect("an HTTP view").request();
        assert_eq!(request.uri().path(), "/x");
    }

    #[test]
    fn a_unit_with_another_view_is_refused_opaquely_and_said_once() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut view = Socket;
        let mut cx = new_unit_context(&ROUTE, &mut view, None, None);
        for _ in 0..2 {
            let refused = HttpView::of(&mut cx).err().expect("no HTTP view");
            assert_eq!(refused.http_status(), 500);
        }
        let event = logs.expect_one(
            "nest_rs::http",
            "an HTTP guard entry ran on a unit with no HTTP view",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("method").as_deref(), Some("route"));
    }
}
