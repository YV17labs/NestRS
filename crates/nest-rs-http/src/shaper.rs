//! Per-route, type-directed response shaping.
//!
//! A route whose handler declares a masking extractor (in practice the
//! `Authorize<_, _>` gate or the `Bind<_, _>` binding) runs inside a
//! [`ResponseShaping`]: the extractor's crate captures what it needs off the
//! request, installs ambient state around the handler, and rewrites the body
//! before it ships.
//!
//! `#[routes]` cannot see through a renamed import (`use Authorize as Az`), so
//! it hands every handler parameter's *type* to [`ShaperProbe`], and the
//! compiler answers whether that type implements [`RouteResponseShaper`].

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use poem::http::StatusCode;
use poem::{Endpoint, Error, IntoResponse, Request, Response, Result};

/// The handler's future, as a [`ResponseShaping`] receives it; boxed on armed
/// routes only.
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
    /// The class gate a route declaring this type runs — `nest_rs_authz`'s
    /// `class_gate` for its `Authorize` — on the unit the route's guard stage
    /// checked, right after its guards: a refusal is answered as the denial it
    /// carries, before any interceptor or filter the route declares, so a layer
    /// that answers on the handler's behalf never answers a caller it refuses.
    #[expect(
        clippy::type_complexity,
        reason = "the pipeline's class gate written out, so a tier-1 signature names no hidden alias"
    )]
    const GATE: Option<
        for<'b> fn(
            &mut nest_rs_core::UnitContext<'b>,
        ) -> std::result::Result<(), nest_rs_core::Failure>,
    > = None;

    /// Snapshot what the shaper needs off the request before the handler takes
    /// ownership of it, or `None` to leave this request's response untouched.
    fn capture(req: &Request) -> Option<Box<dyn ResponseShaping>>;
}

/// The armed capture step, erased to a plain function pointer so a route's
/// shaper can be *selected* by the compiler and *carried* by a value.
pub type CaptureFn = fn(&Request) -> Option<Box<dyn ResponseShaping>>;

/// A route's class gate, as [`RouteResponseShaper::GATE`] declares it: the
/// pipeline's own.
pub type GateFn = nest_rs_core::__private::ClassGate;

/// Asks the compiler whether `T` is a [`RouteResponseShaper`].
///
/// Both arms below answer `select()`; the inherent one is reachable only when
/// its bound holds, and inherent methods win over trait methods at the same
/// autoref step. So [`__shaper_of`](crate::__shaper_of) answers `Some` exactly when
/// `T: RouteResponseShaper`, decided after name resolution.
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

    /// The class gate the armed type declares.
    pub fn gate(&self) -> Option<GateFn> {
        T::GATE
    }
}

/// The fallback arm of the selection: every type that is not a
/// [`RouteResponseShaper`]. Reached by autoref, one step after the inherent
/// [`ShaperProbe::select`].
pub trait UnshapedProbe {
    /// No shaper for this parameter.
    fn select(&self) -> Option<CaptureFn>;

    /// No class gate for this parameter.
    fn gate(&self) -> Option<GateFn>;
}

impl<T> UnshapedProbe for &ShaperProbe<T> {
    fn select(&self) -> Option<CaptureFn> {
        None
    }

    fn gate(&self) -> Option<GateFn> {
        None
    }
}

/// The shaper for one parameter type, or `None`. This is the *only* spelling of
/// the selection: the extra `&` is what makes the two [`ShaperProbe`] arms
/// resolve in the right order, so writing the call out by hand invites a
/// `needless_borrow` "fix" that would quietly disarm the fallback.
#[doc(hidden)]
#[macro_export]
macro_rules! __shaper_of {
    ($ty:ty) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::UnshapedProbe as _;
        #[allow(
            clippy::needless_borrow,
            reason = "the extra borrow is what orders the two probe arms"
        )]
        let __nestrs_probe = &$crate::__private::ShaperProbe::<$ty>::new();
        __nestrs_probe.select()
    }};
}

/// The class gate one parameter type declares, or `None`; selected like
/// [`__shaper_of`](crate::__shaper_of), by the same two probe arms.
#[doc(hidden)]
#[macro_export]
macro_rules! __gate_of {
    ($ty:ty) => {{
        #[allow(
            unused_imports,
            reason = "the trait is in scope for autoref specialisation, which no lint sees"
        )]
        use $crate::__private::UnshapedProbe as _;
        #[allow(
            clippy::needless_borrow,
            reason = "the extra borrow is what orders the two probe arms"
        )]
        let __nestrs_probe = &$crate::__private::ShaperProbe::<$ty>::new();
        __nestrs_probe.gate()
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
/// masking probe's cross-check when nothing armed.
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
    /// The handler, its streaming body continuing under what the handler ran
    /// under.
    async fn plain(&self, req: Request) -> Result<Response> {
        let resp = self.inner.call(req).await?.into_response();
        Ok(crate::response_body::continue_body(resp))
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
    /// The per-request masking probe (see [`MaskProbe`]); extractors run inside
    /// the endpoint's own task, so the scope covers them.
    static MASK_PROBE: Cell<bool>;
}

/// The run-time backstop behind the type-directed arm, for a masking extractor
/// it cannot see (nested in another, or a hand-rolled `FromRequest`): a
/// [`mark`](MaskProbe::mark) on an unarmed route's success fails it closed.
pub struct MaskProbe;

impl MaskProbe {
    /// Record that a masking extractor ran on this request. Called by the
    /// extractors themselves; outside a probe scope (an armed route, another
    /// transport) this is a no-op.
    pub fn mark() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "outside a probe scope the mark is a documented no-op"
        )]
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

    struct NotAShaper;

    #[tokio::test]
    async fn the_probe_selects_a_shaper_by_type_and_nothing_else() {
        assert!(
            __shaper_of!(Shout).is_some(),
            "a RouteResponseShaper arms the route",
        );
        assert!(
            __shaper_of!(NotAShaper).is_none(),
            "any other parameter type leaves it unarmed",
        );
        // An alias, as a renamed import would be.
        type Renamed = Shout;
        assert!(__shaper_of!(Renamed).is_some());
    }

    #[tokio::test]
    async fn an_armed_shaper_wraps_the_handler_and_rewrites_the_body() {
        let ep = shaped(plain, __shaper_of!(Shout), None);
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
        let ep = shaped(plain, __shaper_of!(Shout), None);
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }

    #[tokio::test]
    async fn an_armed_route_runs_no_probe_scope() {
        // A `mark` inside an armed route must not fail it closed.
        static RAN: AtomicBool = AtomicBool::new(false);

        #[handler]
        fn marks() -> &'static str {
            MaskProbe::mark();
            RAN.store(true, Ordering::SeqCst);
            "ok"
        }

        let ep = shaped(marks, __shaper_of!(Shout), Some("GET /armed"));
        let resp = TestClient::new(ep)
            .get("/")
            .header("x-tag", "seen")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("seen:OK").await;
        assert!(RAN.load(Ordering::SeqCst), "the handler ran");
    }

    struct Gated;

    impl RouteResponseShaper for Gated {
        const GATE: Option<GateFn> = Some(|cx| {
            let passes = crate::HttpView::of(cx)?
                .request()
                .header("x-pass")
                .is_some();
            match passes {
                true => Ok(()),
                false => Err(nest_rs_guards::Denial::forbidden("no pass").into()),
            }
        });

        fn capture(_req: &Request) -> Option<Box<dyn ResponseShaping>> {
            None
        }
    }

    static GATED_ROUTE: nest_rs_core::Handler =
        nest_rs_core::__private::declare_handler(nest_rs_core::__private::HandlerDeclaration {
            unit: crate::unit::REQUEST,
            host: std::any::TypeId::of::<Gated>(),
            host_name: "Gated",
            method: "route",
            address: "/gated",
            posture: nest_rs_core::Posture::Gated,
            meta: nest_rs_core::__private::MetaLevels::default,
        });

    /// What `gate` answers on `request`, as the route's unit.
    fn gated(
        gate: GateFn,
        request: crate::Request,
    ) -> std::result::Result<(), nest_rs_core::Failure> {
        let mut view = crate::HttpView::new(request);
        let mut cx = nest_rs_core::__private::new_unit_context(&GATED_ROUTE, &mut view, None, None);
        gate(&mut cx)
    }

    #[test]
    fn the_probe_selects_a_gate_by_type_and_nothing_else() {
        let gate = __gate_of!(Gated).expect("a type declaring a gate selects it");
        let refused =
            gated(gate, crate::Request::builder().finish()).expect_err("no `x-pass`, refused");
        assert_eq!(
            refused
                .find::<nest_rs_guards::Denial>()
                .map(nest_rs_guards::Denial::http_status),
            Some(403),
        );
        let admitted = crate::Request::builder()
            .header(
                crate::HeaderName::from_static("x-pass"),
                crate::HeaderValue::from_static(""),
            )
            .finish();
        assert!(gated(gate, admitted).is_ok());

        assert!(
            __gate_of!(Shout).is_none(),
            "a shaper declares no gate unless it says so"
        );
        assert!(__gate_of!(NotAShaper).is_none());
    }

    #[tokio::test]
    async fn the_first_shaping_parameter_wins() {
        let mut selected: Option<CaptureFn> = None;
        for candidate in [__shaper_of!(NotAShaper), __shaper_of!(Shout)] {
            if selected.is_none() {
                selected = candidate;
            }
        }
        assert!(selected.is_some());
    }
}
