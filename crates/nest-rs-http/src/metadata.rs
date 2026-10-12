//! Handler-declared metadata — the seam a Layer reads at decision time, and
//! the marker a mapped error leaves on the response it produced.
//!
//! A decorator declares a typed value on a host or a handler (`#[meta(EXPR)]`)
//! and its posture (`#[public]`, `#[authorize]`) into the route's one
//! [`Handler`]; a Layer (Guard / Interceptor / Filter / Pipe) reads it back at
//! request time through [`HandlerMetadata`], whose one implementor is
//! [`Reflector`](crate::Reflector).

use std::any::Any;

use nest_rs_core::{Handler, Posture};

/// Typed read access to whatever metadata was attached to the current handler.
pub trait HandlerMetadata {
    /// Returns the attached value of type `M`, or `None` when nothing of
    /// that type was attached at this handler. Lookup is by
    /// [`TypeId`](std::any::TypeId): wrap same-shaped values in distinct newtypes.
    fn get<M: Any + Send + Sync>(&self) -> Option<&M>;

    /// Whether the handler was marked `#[public]`, read off the [`Public`] marker.
    fn is_public(&self) -> bool {
        self.get::<Public>().is_some()
    }

    /// Who may call the handler: the [`Posture`] its mount stamped
    /// (`Posture::InBand` on `/graphql`), else `Public` under the [`Public`]
    /// marker, else `Gated`.
    fn posture(&self) -> Posture {
        match self.get::<Posture>() {
            Some(stamped) => *stamped,
            None if self.is_public() => Posture::Public,
            None => Posture::Gated,
        }
    }
}

/// The `#[public]` marker on a request no route declared — one an endpoint or a
/// test builds by hand; a route's own posture is its [`Handler`]'s. The
/// framework does **not** act on it — guards read the posture through
/// [`HandlerMetadata::posture`] and decide whether to honor it. An
/// authentication guard admits an anonymous caller on `Posture::InBand` too,
/// the posture of `/graphql`, whose operations each declare their own.
///
/// ```
/// # use nest_rs_guards::prelude::*;
/// # use nest_rs_http::Reflector;
/// # struct ApiKeyGuard;
/// # impl Layer for ApiKeyGuard {}
/// # #[async_trait]
/// # impl Guard for ApiKeyGuard {
/// // In a guard:
/// async fn check_http(&self, req: &mut HttpRequest) -> Result<(), Denial> {
///     if matches!(Reflector::new(req).posture(), Posture::Public | Posture::InBand) {
///         return Ok(());
///     }
///     // ...standard policy...
/// #   Err(Denial::unauthorized("an API key is required"))
/// }
/// # }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let mut public = HttpRequest::default();
/// # public.extensions_mut().insert(Public);
/// # let mut in_band = HttpRequest::default();
/// # in_band.extensions_mut().insert(Posture::InBand);
///
/// assert!(ApiKeyGuard.check_http(&mut public).await.is_ok());
/// assert!(ApiKeyGuard.check_http(&mut in_band).await.is_ok());
/// assert!(ApiKeyGuard.check_http(&mut HttpRequest::default()).await.is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Public;

/// The route's [`Handler`], carried on each request its route shaper runs, so
/// a layer reads what the route declared through a pointer: nothing is copied
/// per request, and nothing a layer inserts shadows it.
#[derive(Clone, Copy, Debug)]
pub struct HandlerRef(pub &'static Handler);

/// Marker inserted into a **response**'s extensions when that response was
/// produced by mapping a handler error — a route-site `Filter` or a matching
/// `ExceptionFilter` turned an `Err` into a `Response`.
///
/// The data layer rolls the ambient transaction back on it, even when the
/// mapped status is a success.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MappedError;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reflector;
    use poem::Request;

    #[test]
    fn a_stamped_posture_is_read_as_stamped() {
        let mut req = Request::default();
        req.extensions_mut().insert(Posture::InBand);
        assert_eq!(Reflector::new(&req).posture(), Posture::InBand);
        assert!(!Reflector::new(&req).is_public());
    }

    #[test]
    fn the_public_marker_reads_as_public() {
        let mut req = Request::default();
        req.extensions_mut().insert(Public);
        assert_eq!(Reflector::new(&req).posture(), Posture::Public);
    }

    #[test]
    fn a_handler_declaring_nothing_is_gated() {
        assert_eq!(
            Reflector::new(&Request::default()).posture(),
            Posture::Gated
        );
    }
}
