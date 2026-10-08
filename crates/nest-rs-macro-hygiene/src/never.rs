//! [`Never`] — the error a handler that cannot fail returns.

/// `!`, named on stable: every decorator's expansion compiles a handler
/// returning `Result<T, Never>` without an unreachable call.
pub type Never = <fn() -> ! as Returns>::Output;

/// What a function pointer returns — the stable path to naming `!`.
pub trait Returns {
    /// The return type.
    type Output;
}

impl<T> Returns for fn() -> T {
    type Output = T;
}
