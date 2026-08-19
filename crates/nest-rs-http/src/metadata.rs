//! Handler-attached metadata — the seam a Layer reads at decision time, and
//! the marker a mapped error leaves on the response it produced.
//!
//! A decorator attaches a typed value to a handler at mount time
//! (`#[meta(EXPR)]`, `#[public]`); a Layer (Guard / Interceptor / Filter /
//! Pipe) reads it back at request time through [`HandlerMetadata`], whose one
//! implementor is [`Reflector`](crate::Reflector).
//!
//! **Why this is HTTP's and not the kernel's.** It reads a value out of a
//! `poem::Request` and marks one on a `poem::Response` — two types the kernel
//! cannot name — and the trait carried a promise the other edges never took
//! up: they resolve posture at *compile* time, through the `Posture` the
//! impl-half decorator parses, so a WS message and an MCP tool have no
//! per-handler data slot for a Layer to consult and never needed one. It lived
//! in `nest-rs-core` as a transport-agnostic contract with a single
//! implementor for as long as that promise stood unread.
//!
//! Every reader already depends on this crate unconditionally — `nest-rs-guards`
//! by the argument recorded in `framework.md`, and the four Layer families
//! through it — so nothing pays a dependency for the move.

use std::any::Any;

/// Typed read access to whatever metadata was attached to the current handler.
///
/// The contract is intentionally minimal: a single typed lookup. The
/// [`is_public`](Self::is_public) default reads the framework's only universal
/// marker; everything else is a Layer-local concern.
pub trait HandlerMetadata {
    /// Returns the attached value of type `M`, or `None` when nothing of
    /// that type was attached at this handler. Implementations resolve by
    /// [`TypeId`](std::any::TypeId) — wrap multiple values of the same
    /// underlying shape in distinct newtypes when they need to coexist.
    fn get<M: Any + Send + Sync>(&self) -> Option<&M>;

    /// Whether the handler was marked `#[public]`. Default reads the
    /// [`Public`] marker; implementors rarely override.
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
