//! Per-route, type-directed response shaping.
//!
//! A route declaring a shaping type, a parameter's or the posture's, runs inside a
//! [`ResponseShaping`]: the type's crate captures what it needs off the request,
//! installs ambient state around the handler, and may rewrite the body before it
//! ships. The `Authorize<_, _>` an `#[authorize]` or a `#[public(Action, Subject)]`
//! emits declares the class gate alone and captures nothing: each wire row a reply
//! carries was judged where it was built.
//!
//! `#[routes]` cannot see through a renamed import, so it hands the posture's type
//! and every handler parameter's to [`ShaperProbe`], and the compiler answers
//! whether that type implements [`RouteResponseShaper`].

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use futures_util::FutureExt as _;
use futures_util::future::Either;

use crate::endpoint::Endpoint;
use crate::error::Result;
use crate::request::Request;
use crate::response::Response;
use crate::route_site::RouteWrapper;

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

    /// Whether a route declaring this type arms its `capture`, and its document
    /// says a nullable property of its reply reads `null` where a grant withholds
    /// it. `false` for a type that only declares [`GATE`](Self::GATE) — a posture
    /// on a subject with no rows — whose `capture` never runs and which an
    /// `#[sse]` route may declare.
    const SHAPES: bool = true;

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
/// `T: RouteResponseShaper` shapes, decided after name resolution, and
/// [`__gate_of`](crate::__gate_of) the gate such a type declares, shaping or not.
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
    /// The armed arm: the type's capture step, unless it only gates.
    pub fn select(&self) -> Option<CaptureFn> {
        T::SHAPES.then_some(T::capture as CaptureFn)
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
/// handler: `shaper` is the type-directed selection over the parameter list.
pub fn shaped<E>(inner: E, shaper: Option<CaptureFn>) -> ShapedEndpoint<E> {
    ShapedEndpoint { inner, shaper }
}

/// The route's shaping seam: applies the armed [`ResponseShaping`], if any.
pub struct ShapedEndpoint<E> {
    inner: E,
    shaper: Option<CaptureFn>,
}

/// `handler`'s answer, its streaming body continuing under what the handler
/// ran under, shaped by `shaping` when one is armed.
///
/// Not an `async fn`, nor a block awaiting `handler`: either keeps the future
/// it is handed beside the one it awaits, so each would double the handler's
/// future, and a debug build reserves every copy on the polling thread's stack.
fn shape<'a>(
    handler: impl Future<Output = Result<Response>> + Send + 'a,
    shaping: Option<Box<dyn ResponseShaping>>,
) -> impl Future<Output = Result<Response>> + Send + 'a {
    let plain = handler.map(|resp| resp.map(crate::response_body::continue_body));
    match shaping {
        Some(shaping) => Either::Left(shaping.apply(Box::pin(plain))),
        None => Either::Right(plain),
    }
}

impl<E: Endpoint> Endpoint for ShapedEndpoint<E> {
    async fn call(&self, req: Request) -> Result<Response> {
        let shaping = self.shaper.and_then(|capture| capture(&req));
        shape(self.inner.call(req), shaping).await
    }
}

impl<W: RouteWrapper> RouteWrapper for ShapedEndpoint<W> {
    async fn call(&self, req: &mut Request) -> Result<Response> {
        let shaping = self.shaper.and_then(|capture| capture(req));
        shape(self.inner.call(req), shaping).await
    }
}

#[cfg(test)]
mod tests {
    use http::StatusCode;

    use crate::endpoint::endpoint_fn;
    use crate::testing::{Holding, get};
    use crate::{HeaderName, HeaderValue};

    use super::*;

    fn plain() -> impl Endpoint {
        endpoint_fn(|_req| async { Ok(Response::builder().body("ok")) })
    }

    /// `ep`'s answer to a request, `x-tag` set when `tag` is.
    async fn answer(ep: &impl Endpoint, tag: Option<&'static str>) -> Result<Response> {
        let mut req = Request::builder();
        if let Some(tag) = tag {
            req = req.header(
                HeaderName::from_static("x-tag"),
                HeaderValue::from_static(tag),
            );
        }
        ep.call(req.finish()).await
    }

    async fn text(resp: Result<Response>) -> (StatusCode, String) {
        let resp = resp.expect("answers");
        let status = resp.status();
        (
            status,
            resp.into_body().into_string().await.expect("a body"),
        )
    }

    #[tokio::test]
    async fn an_unarmed_route_passes_the_response_through() {
        let ep = shaped(plain(), None);
        assert_eq!(
            text(answer(&ep, None).await).await,
            (StatusCode::OK, "ok".into())
        );
    }

    struct Shout;

    struct Shouting(String);

    impl ResponseShaping for Shouting {
        fn apply<'a>(self: Box<Self>, inner: RouteFuture<'a>) -> RouteFuture<'a> {
            Box::pin(async move {
                let resp = inner.await?;
                let body = resp.into_body().into_string().await.unwrap_or_default();
                Ok(Response::builder().body(format!("{}:{}", self.0, body.to_uppercase())))
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
        let ep = shaped(plain(), __shaper_of!(Shout));
        assert_eq!(
            text(answer(&ep, Some("seen")).await).await,
            (StatusCode::OK, "seen:OK".into())
        );
    }

    #[tokio::test]
    async fn a_capture_that_declines_leaves_the_response_untouched() {
        let ep = shaped(plain(), __shaper_of!(Shout));
        assert_eq!(
            text(answer(&ep, None).await).await,
            (StatusCode::OK, "ok".into())
        );
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

    /// A type that only gates: a posture on a subject with no rows.
    struct GatesOnly;

    impl RouteResponseShaper for GatesOnly {
        const GATE: Option<GateFn> = Gated::GATE;
        const SHAPES: bool = false;

        fn capture(_req: &Request) -> Option<Box<dyn ResponseShaping>> {
            None
        }
    }

    #[test]
    fn a_type_that_only_gates_selects_its_gate_and_arms_no_shaper() {
        assert!(
            __gate_of!(GatesOnly).is_some(),
            "its gate runs with the guards"
        );
        assert!(
            __shaper_of!(GatesOnly).is_none(),
            "nothing shapes its route, so the route reads as unmasked"
        );
        assert!(__shaper_of!(Gated).is_some(), "a type shapes by default");
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

    /// What shaping adds to the future of a handler holding `N` bytes, as
    /// `#[routes]` mounts it and as an endpoint.
    fn added_by_shaping<const N: usize>() -> [usize; 2] {
        let handler = size_of_val(&RouteWrapper::call(&Holding::<N>, &mut get()));
        let ep = shaped(Holding::<N>, __shaper_of!(Shout));
        [
            size_of_val(&RouteWrapper::call(&ep, &mut get())) - handler,
            size_of_val(&Endpoint::call(&ep, get())) - handler,
        ]
    }

    #[test]
    fn shaping_holds_the_handlers_future_once() {
        let light = added_by_shaping::<1024>();
        let heavy = added_by_shaping::<16384>();
        assert_eq!(
            light, heavy,
            "shaping copies the handler's future: it adds {light:?} bytes to a 1 KiB one, \
             {heavy:?} to a 16 KiB one",
        );
    }
}
