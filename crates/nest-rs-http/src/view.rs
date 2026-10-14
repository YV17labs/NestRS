//! [`HttpView`], the HTTP edge's view of the request it dispatches.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

use nest_rs_core::{UnitContext, UnitView};
use nest_rs_guards::Denial;

use crate::request::Request;

/// The HTTP edge's view of one request: what a guard reads through
/// [`HttpView::of`] — the request, head and body — and the route it matched.
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
pub struct HttpView {
    request: Request,
}

impl UnitView for HttpView {}

impl HttpView {
    /// The view of `request`, as the edge builds it for its guard stage.
    pub(crate) fn new(request: Request) -> Self {
        Self { request }
    }

    /// The request back, once the stage that read it is over.
    pub(crate) fn into_request(self) -> Request {
        self.request
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

    /// The request: its head, and its body until a reader takes it.
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
}

/// `view`'s request as poem's, taken out of it for a reader still typed on
/// poem (an authentication `Strategy`), and put back with [`put_poem`].
pub fn take_poem(view: &mut HttpView) -> poem::Request {
    let request = std::mem::replace(&mut view.request, Request::builder().finish());
    crate::poem_bridge::request_to_poem(request)
}

/// The request [`take_poem`] took, back in `view`.
pub fn put_poem(view: &mut HttpView, request: poem::Request) {
    view.request = crate::poem_bridge::request_from_poem(request);
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
