//! [`McpAccess`] — who may call the app's MCP endpoints, the one opening
//! declared in code.

/// Who may call the app's MCP endpoints, when the app opens them: the one
/// opening an MCP endpoint takes, so no other posture compiles.
///
/// An endpoint is gated unless an import opens it: at each request it runs the
/// global guard pool and the `EndpointGuards<McpView, _>` the app's modules
/// declare, and with neither it denies every request. [`Public`](Self::Public)
/// opens it with no guard at the request — the one visible line that does, in
/// code alone, at one import: `McpModule::for_root(McpAccess::Public)`. A
/// second import opening them fails the boot naming both, and a module
/// declaring `EndpointGuards` for them fails it too.
///
/// Gated has no spelling: it is what every import leaves, so no line pins it
/// and no line reads as a closing another import's opening would override.
///
/// ```compile_fail,E0599
/// use nest_rs_mcp::{McpAccess, McpModule};
///
/// let _ = McpModule::for_root(McpAccess::Gated);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the one opening an MCP endpoint takes is the whole set: another would be a \
              posture the boot refuses, which this type exists to keep from compiling"
)]
pub enum McpAccess {
    /// The request runs no guard: every operation's own posture still holds.
    Public,
}
