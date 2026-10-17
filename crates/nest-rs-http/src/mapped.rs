//! The marker a mapped error leaves on the response it produced.

/// Marker inserted into a **response**'s extensions when that response was
/// produced by mapping a handler error — a route-site `Filter` or a matching
/// `ExceptionFilter` turned an `Err` into a `Response`.
///
/// The data layer rolls the ambient transaction back on it, even when the
/// mapped status is a success.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MappedError;
