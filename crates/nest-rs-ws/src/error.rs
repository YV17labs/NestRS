//! Typed errors for the WebSocket edge.

/// Why a request-scoped provider could not be resolved inside a WS message
/// handler. `Display` is what the `#[messages]` reply mapping puts on the error
/// frame, so a handler can `?` it directly.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WsScopeError {
    /// The per-message request scope was not installed — the handler ran off the
    /// dispatch task, or the gateway is not nested under the HTTP request scope.
    #[error("request scope not installed — the WS gateway installs it per message")]
    NoScope,
    /// No provider of the requested type is registered in any reachable module.
    #[error("no provider registered for `{0}` — add it to a module's providers")]
    NoProvider(&'static str),
}
