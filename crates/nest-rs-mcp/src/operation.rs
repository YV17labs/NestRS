//! What one MCP operation *is*, for the layers that run around it.
//!
//! An HTTP handler is handed a `Request` and a mounted `RouteShaper`; a GraphQL
//! resolver is handed a `Context` carrying the [`Container`]. An MCP operation
//! is handed neither: rmcp builds the host once per session and dispatches each
//! operation on its own spawned task, so a generated prelude has nothing to read
//! the app off. This module is that seam.
//!
//! [`McpOperationContext`] is what a [`Guard`] then sees. It deliberately does
//! **not** carry the operation's arguments: deciding *access* from a payload is
//! a pipe's job (`Valid<T>` / `Piped<P, T>` run on the wire value before the
//! body), and handing a guard the arguments invites the check to migrate into
//! the one place the layer rules say it must not be.

use std::fmt;

use nest_rs_core::Container;

/// The container serving the current MCP operation, if one is installed.
///
/// Read off the ambient request scope rather than carried again: the HTTP
/// transport edge builds that scope with the app container and the MCP endpoint
/// nests under it, so a second copy would be one more thing two installs have to
/// agree about. `None` outside an MCP dispatch, and inside one whose mount is
/// not nested under the transport edge.
pub fn current_container() -> Option<Container> {
    crate::scope::current_scope().map(|scope| scope.root().clone())
}

/// Which router an operation belongs to — the two roles `#[tools]` serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpOperationKind {
    /// A `#[tool]` method, reached by `tools/call`.
    Tool,
    /// A `#[prompt]` method, reached by `prompts/get`.
    Prompt,
}

impl McpOperationKind {
    /// The lowercase word this role is logged and labelled under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Prompt => "prompt",
        }
    }
}

impl fmt::Display for McpOperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One MCP operation, as a [`Guard`](https://docs.rs/nest-rs-guards) sees it —
/// the MCP analog of the `&Context` a `check_graphql` takes and the
/// `(client, event, data)` a `check_ws_message` takes.
///
/// Built by the `#[tools]` expansion around each decorated operation. The caller's
/// `Ability` is **ambient** by the time a guard runs (the endpoint's
/// [`McpOperationGuard`](crate::McpOperationGuard) installs it in its `around`),
/// so a capability-only guard reads it with `nest_rs_authz::current_ability`
/// exactly as it would on any other transport.
pub struct McpOperationContext<'a> {
    container: &'a Container,
    host: &'static str,
    kind: McpOperationKind,
    name: &'static str,
}

impl<'a> McpOperationContext<'a> {
    /// Describe the operation about to run. Macro-emitted; the arguments come
    /// from the decorated method, so every field is `'static` but the container.
    pub fn new(
        container: &'a Container,
        host: &'static str,
        kind: McpOperationKind,
        name: &'static str,
    ) -> Self {
        Self {
            container,
            host,
            kind,
            name,
        }
    }

    /// The app serving this operation — what a guard resolves collaborators
    /// from when it needs one it did not `#[inject]`.
    pub fn container(&self) -> &Container {
        self.container
    }

    /// The `#[mcp]` host type this operation belongs to.
    pub fn host(&self) -> &'static str {
        self.host
    }

    /// Whether this is a tool call or a prompt fetch.
    pub fn kind(&self) -> McpOperationKind {
        self.kind
    }

    /// The operation's wire name — the tool or prompt the client asked for.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl fmt::Debug for McpOperationContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOperationContext")
            .field("host", &self.host)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}
