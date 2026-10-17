//! [`McpView`], the MCP edge's view of the operation it dispatches.

use nest_rs_core::UnitView;

use crate::operation::McpOperationKind;

/// The MCP edge's view of one operation a guard checks: the JSON-RPC method,
/// which router serves it, the tool or prompt it names, and its `#[mcp]` host.
///
/// A guard reaches it through `UnitContext::switch_to` in `Guard::check`. It
/// carries no arguments: deciding access from a payload is a pipe's job, never
/// a guard's.
#[derive(Clone, Copy, Debug)]
pub struct McpView {
    kind: McpOperationKind,
    name: &'static str,
    host: &'static str,
}

impl UnitView for McpView {}

impl McpView {
    /// The view of the operation `name` on `host`, served by `kind`'s router.
    pub(crate) fn new(kind: McpOperationKind, name: &'static str, host: &'static str) -> Self {
        Self { kind, name, host }
    }

    /// The JSON-RPC method the client called (`tools/call`, `prompts/get`).
    pub fn method(&self) -> &'static str {
        match self.kind {
            McpOperationKind::Tool => "tools/call",
            McpOperationKind::Prompt => "prompts/get",
        }
    }

    /// Whether this is a tool call or a prompt fetch.
    pub fn kind(&self) -> McpOperationKind {
        self.kind
    }

    /// The tool or prompt the client named.
    pub fn name(&self) -> Option<&str> {
        Some(self.name)
    }

    /// The `#[mcp]` host type the operation belongs to.
    pub fn host(&self) -> &'static str {
        self.host
    }
}
