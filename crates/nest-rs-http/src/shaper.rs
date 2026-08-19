//! Per-route, type-directed response shaping.
//!
//! A route whose handler declares a masking extractor (in practice the
//! `Authorize<_, _>` gate or the `Bind<_, _>` binding) runs inside a
//! [`ResponseShaping`]: the extractor's crate captures what it needs off the
//! request, installs ambient state around the handler, and rewrites the body
//! before it ships.
//!
//! **Arming is a question for the compiler, not for the macro.** `#[routes]`
//! cannot see through a renamed import (`use Authorize as Az`), so it does not
//! try: it hands every handler parameter's *type* to [`ShaperProbe`], and the
//! compiler answers whether that type implements [`RouteResponseShaper`]. The
//! name the developer chose can no longer change the answer — which is what
//! makes the arm alias-proof.
//!
//! The trait is implemented outside this crate (`nest_rs_authz::http`,
//! `nest_rs_seaorm::http`) so the HTTP surface stays unaware of any specific
//! shaper.

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use poem::http::StatusCode;
use poem::{Endpoint, Error, IntoResponse, Request, Response, Result};

/// The handler's future, as a [`ResponseShaping`] receives it. Boxed because
/// the shaping step is selected by value (a function pointer picked per
/// parameter type) rather than by monomorphising the whole endpoint — one
/// allocation, on armed routes only.
pub type RouteFuture<'a> = Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>>;

/// A shaper's request-independent half: whatever [`RouteResponseShaper::capture`]
/// snapshotted off the request, ready to wrap the handler.
///
/// [`apply`](Self::apply) may both install ambient state for the handler's
/// duration and transform the response it returns.
pub trait ResponseShaping: Send + 'static {
    /// Run `inner` and shape its result.
    fn apply<'a>(self: Box<Self>, inner: RouteFuture<'a>) -> RouteFuture<'a>;
}

/// A type that shapes the response of any route declaring it as a parameter.
///
/// `#[routes]` arms this by *type*: see the module docs.
pub trait RouteResponseShaper {
    /// Snapshot what the shaper needs off the request before the handler takes
    /// ownership of it, or `None` to leave this request's response untouched.
    fn capture(req: &Request) -> Option<Box<dyn ResponseShaping>>;
}

/// The armed capture step, erased to a plain function pointer so a route's
/// shaper can be *selected* by the compiler and *carried* by a value.
pub type CaptureFn = fn(&Request) -> Option<Box<dyn ResponseShaping>>;

/// Asks the compiler whether `T` is a [`RouteResponseShaper`].
///
/// Both arms below answer `select()`; the inherent one is reachable only when
/// its bound holds, and inherent methods win over trait methods at the same
/// autoref step. So [`shaper_of`](crate::shaper_of) answers `Some` exactly when
/// `T: RouteResponseShaper`, decided after name resolution — the property the
/// old path-segment scan could not have.
pub struct ShaperProbe<T>(PhantomData<fn() -> T>);

impl<T> Default for ShaperProbe<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ShaperProbe<T> {
    /// The probe value the selection expression takes a reference to.
    pub const fn new() -> Self {
        ShaperProbe(PhantomData)
    }
}

impl<T: RouteResponseShaper> ShaperProbe<T> {
    /// The armed arm.
    pub fn select(&self) -> Option<CaptureFn> {
        Some(T::capture)
    }
}

/// The fallback arm of the selection: every type that is not a
/// [`RouteResponseShaper`]. Reached by autoref, one step after the inherent
/// [`ShaperProbe::select`].
pub trait UnshapedProbe {
    /// No shaper for this parameter.
    fn select(&self) -> Option<CaptureFn>;
}

impl<T> UnshapedProbe for &ShaperProbe<T> {
    fn select(&self) -> Option<CaptureFn> {
        None
    }
}

/// The shaper for one parameter type, or `None`. This is the *only* spelling of
/// the selection: the extra `&` is what makes the two [`ShaperProbe`] arms
/// resolve in the right order, so writing the call out by hand invites a
/// `needless_borrow` "fix" that would quietly disarm the fallback.
///
/// Emitted by `#[routes]`, once per handler parameter — a cross-crate seam, not
/// public API.
#[doc(hidden)]
#[macro_export]
macro_rules! shaper_of {
    ($ty:ty) => {{
        #[allow(unused_imports)]
        use $crate::UnshapedProbe as _;
        #[allow(clippy::needless_borrow)]
        let __nestrs_probe = &$crate::ShaperProbe::<$ty>::new();
        __nestrs_probe.select()
    }};
}

/// Wrap `inner` with the route's shaping seam. Emitted by `#[routes]` for every
/// handler: `shaper` is the type-directed selection over the parameter list,
/// and `probe` names the route when it carries extractors (see [`MaskProbe`]).
pub fn shaped<E>(
    inner: E,
    shaper: Option<CaptureFn>,
    probe: Option<&'static str>,
) -> ShapedEndpoint<E> {
    ShapedEndpoint {
        inner,
        shaper,
        probe,
    }
}

/// The route's shaping seam: applies the armed [`ResponseShaping`], or runs the
/// [`MaskProbe`] cross-check when nothing armed.
pub struct ShapedEndpoint<E> {
    inner: E,
    shaper: Option<CaptureFn>,
    probe: Option<&'static str>,
}

impl<E> ShapedEndpoint<E>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    async fn plain(&self, req: Request) -> Result<Response> {
        self.inner.call(req).await.map(IntoResponse::into_response)
    }

    async fn probed(&self, req: Request, route: &'static str) -> Result<Response> {
        let (marked, resp) = MASK_PROBE
            .scope(Cell::new(false), async {
                let resp = self.plain(req).await;
                (MASK_PROBE.with(Cell::get), resp)
            })
            .await;
        let resp = resp?;
        if marked && resp.status().is_success() {
            tracing::error!(
                target: crate::target::HTTP,
                route = route,
                "a masking extractor ran but no response shaper armed on this route — \
                 declare it as a handler parameter (a nested or hand-rolled extractor \
                 is invisible to the type-directed arm); failing closed",
            );
            return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
        Ok(resp)
    }
}

impl<E> Endpoint for ShapedEndpoint<E>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        match self.shaper.and_then(|capture| capture(&req)) {
            Some(shaping) => {
                let inner: RouteFuture<'_> = Box::pin(self.plain(req));
                shaping.apply(inner).await
            }
            None => match self.probe {
                Some(route) => self.probed(req, route).await,
                None => self.plain(req).await,
            },
        }
    }
}

tokio::task_local! {
    /// The per-request masking probe (see [`MaskProbe`]). A task-local `Cell`
    /// instead of a request extension: zero heap allocation per request, and
    /// extractors run inside the endpoint's own task so the scope provably
    /// covers them — the same pattern as the ambient executor and ability.
    static MASK_PROBE: Cell<bool>;
}

/// The run-time backstop behind the type-directed arm.
///
/// The arm answers on a *parameter's type*, so it covers every way a masking
/// extractor is normally written, renamed imports included. What it cannot see
/// is an extractor reached indirectly — one nested inside another extractor, or
/// a hand-rolled `FromRequest` that runs the gate itself. Those routes arm
/// nothing, so they run inside a probe scope
/// ([`ShapedEndpoint`]): a [`mark`](MaskProbe::mark) on a success response
/// fails the request closed instead of shipping unmasked fields.
pub struct MaskProbe;

impl MaskProbe {
    /// Record that a masking extractor ran on this request. Called by the
    /// extractors themselves; outside a probe scope (an armed route, another
    /// transport) this is a no-op.
    pub fn mark() {
        let _ = MASK_PROBE.try_with(|marked| marked.set(true));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use poem::handler;
    use poem::test::TestClient;

    use super::*;

    #[handler]
    fn marks_and_succeeds() -> &'static str {
        MaskProbe::mark();
        "unmasked body"
    }

    #[handler]
    fn plain() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn a_marked_probe_on_success_fails_closed() {
        let logs = nest_rs_testing::LogCapture::install();
        let ep = shaped(marks_and_succeeds, None, Some("GET /users"));
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

        // The 500 is deliberately opaque, so the event is the only place the
        // cause exists — and the cause is a *code* mistake, not an outage: the
        // route runs a masking extractor the type-directed arm never saw, so it
        // would have shipped unmasked columns. Without the route in the fields
        // an operator has a bare 500 and no way back to the handler.
        let event = logs.expect_one(
            "nest_rs::http",
            "a masking extractor ran but no response shaper armed on this route — \
             declare it as a handler parameter (a nested or hand-rolled extractor \
             is invisible to the type-directed arm); failing closed",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("route").as_deref(), Some("GET /users"));
    }

    #[tokio::test]
    async fn an_unmarked_probe_passes_the_response_through() {
        let ep = shaped(plain, None, Some("GET /health"));
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }

    /// A shaper reads the request before the handler consumes it, wraps the
    /// handler, and rewrites the body — the three things `#[routes]` relies on.
    struct Shout;

    struct Shouting(String);

    impl ResponseShaping for Shouting {
        fn apply<'a>(self: Box<Self>, inner: RouteFuture<'a>) -> RouteFuture<'a> {
            Box::pin(async move {
                let resp = inner.await?;
                let body = resp.into_body().into_string().await.unwrap_or_default();
                Ok(format!("{}:{}", self.0, body.to_uppercase()).into_response())
            })
        }
    }

    impl RouteResponseShaper for Shout {
        fn capture(req: &Request) -> Option<Box<dyn ResponseShaping>> {
            let tag = req.header("x-tag")?;
            Some(Box::new(Shouting(tag.to_owned())))
        }
    }

    /// Stands in for every handler parameter that is not a shaper.
    struct NotAShaper;

    #[tokio::test]
    async fn the_probe_selects_a_shaper_by_type_and_nothing_else() {
        assert!(
            shaper_of!(Shout).is_some(),
            "a RouteResponseShaper arms the route",
        );
        assert!(
            shaper_of!(NotAShaper).is_none(),
            "any other parameter type leaves it unarmed",
        );
        // The name the parameter is written under is not part of the answer.
        type Renamed = Shout;
        assert!(shaper_of!(Renamed).is_some());
    }

    #[tokio::test]
    async fn an_armed_shaper_wraps_the_handler_and_rewrites_the_body() {
        let ep = shaped(plain, shaper_of!(Shout), None);
        let resp = TestClient::new(ep)
            .get("/")
            .header("x-tag", "seen")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("seen:OK").await;
    }

    #[tokio::test]
    async fn a_capture_that_declines_leaves_the_response_untouched() {
        // No `x-tag`: the shaper captures nothing, so the route falls through
        // to the unshaped path rather than half-applying.
        let ep = shaped(plain, shaper_of!(Shout), None);
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }

    #[tokio::test]
    async fn an_armed_route_runs_no_probe_scope() {
        // The probe is the fallback net, not a second layer: an armed route
        // must not pay for it, and a `mark` inside one must not fail it closed.
        static RAN: AtomicBool = AtomicBool::new(false);

        #[handler]
        fn marks() -> &'static str {
            MaskProbe::mark();
            RAN.store(true, Ordering::SeqCst);
            "ok"
        }

        let ep = shaped(marks, shaper_of!(Shout), Some("GET /armed"));
        let resp = TestClient::new(ep)
            .get("/")
            .header("x-tag", "seen")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("seen:OK").await;
        assert!(RAN.load(Ordering::SeqCst), "the handler ran");
    }

    /// The selection is a value, so a route with several parameters picks the
    /// first shaper among them exactly as `#[routes]` folds them.
    #[tokio::test]
    async fn the_first_shaping_parameter_wins() {
        let mut selected: Option<CaptureFn> = None;
        for candidate in [shaper_of!(NotAShaper), shaper_of!(Shout)] {
            if selected.is_none() {
                selected = candidate;
            }
        }
        assert!(selected.is_some());
    }
}
