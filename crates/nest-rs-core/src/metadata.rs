//! Handler-attached metadata — the transport-agnostic seam Layers read at
//! decision time.
//!
//! A decorator macro (e.g. `#[meta(EXPR)]`) attaches a typed value to a
//! handler at mount time. A Layer (Guard / Interceptor / Filter / Pipe) needs
//! to read that value at request time *without* knowing which transport
//! delivered the request. Each transport provides its own [`HandlerMetadata`]
//! implementation over its native request type (HTTP's `Reflector`, a WS
//! gateway's per-message data slot, an MCP tool's call context). Layers
//! depend on the trait — never on a specific transport's reader — so the same
//! Layer reads metadata identically over HTTP, GraphQL, WS, MCP, …
//!
//! Why this lives in core: it is the *contract* every transport needs to
//! satisfy so Layers stay portable. Putting it elsewhere would force a Layer
//! to either drop down to per-transport readers or pull in a transport
//! dependency.

use std::any::Any;

/// Typed read access to whatever metadata was attached to the current
/// handler. Each transport implements it over its native request type.
///
/// The contract is intentionally minimal: a single typed lookup. The
/// [`is_public`](Self::is_public) default reads the framework's only
/// universal marker; everything else is a Layer-local concern.
pub trait HandlerMetadata {
    /// Returns the attached value of type `M`, or `None` when nothing of
    /// that type was attached at this handler. Implementations resolve by
    /// [`TypeId`](std::any::TypeId) — wrap multiple values of the same
    /// underlying shape in distinct newtypes when they need to coexist.
    fn get<M: Any + Send + Sync>(&self) -> Option<&M>;

    /// Whether the handler was marked `#[public]`. Default reads the
    /// [`Public`] marker; transports rarely override.
    fn is_public(&self) -> bool {
        self.get::<Public>().is_some()
    }
}

/// Marker attached as handler metadata when a handler is `#[public]`. The
/// framework does **not** act on it — guards read it through
/// [`HandlerMetadata::is_public`] and decide whether to honor it.
///
/// ```rust,ignore
/// // In a guard:
/// fn check_http(&self, req: &mut HttpRequest) -> Result<(), Denial> {
///     if Reflector::new(req).is_public() {
///         return Ok(());
///     }
///     // ...standard policy...
/// }
/// ```
///
/// Lives here (not in `layer`) because it is *an example of handler
/// metadata*, not part of the Layer vocabulary itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Public;

/// Marker inserted into a **response**'s extensions when that response was
/// produced by mapping a handler error — a route-site `Filter` or a matching
/// `ExceptionFilter` turned an `Err` into a `Response`.
///
/// The data layer reads it to keep transactional integrity: the handler
/// *failed*, so whatever it wrote inside the ambient transaction is suspect —
/// the transaction must roll back even when the mapped response carries a
/// success status. Without this marker, a per-route filter mapping an error
/// to `200` would silently commit a half-applied mutation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MappedError;
