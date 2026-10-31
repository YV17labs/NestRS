//! [`McpAccess`] — who may call the app's MCP endpoints, the one opening
//! declared in code.

/// Who may call the app's MCP endpoints: the two postures an MCP endpoint
/// takes, so no other compiles.
///
/// [`Gated`](Self::Gated), the default, runs the global guard pool and the
/// `EndpointGuards<McpView, _>` the app's modules declare at each request, and
/// with neither the endpoint denies every request. [`Public`](Self::Public)
/// opens them with no guard at the request — the one visible line that does,
/// in code alone, at one import: `McpModule::for_root(McpAccess::Public)`. A
/// second import opening them fails the boot naming both, and a module
/// declaring `EndpointGuards` for them fails it too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the two postures an MCP endpoint takes are the whole set: a third would be a \
              posture the boot refuses, which this type exists to keep from compiling"
)]
pub enum McpAccess {
    /// The request runs the guards the app declares, and is denied with none.
    #[default]
    Gated,
    /// The request runs no guard: every operation's own posture still holds.
    Public,
}
