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

use std::any::TypeId;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use nest_rs_core::Container;

tokio::task_local! {
    static MCP_ALREADY_RAN: Arc<[TypeId]>;
}

/// Run `fut` with the endpoint guard's report ambient for the operation.
///
/// Installed by [`PropagatingHandler`](crate::PropagatingHandler) beside the
/// request scope, from a set the mount computed once — see
/// [`layers_already_run`].
pub(crate) async fn with_already_run<F: Future>(
    already_ran: Option<Arc<[TypeId]>>,
    fut: F,
) -> F::Output {
    match already_ran {
        Some(already_ran) => MCP_ALREADY_RAN.scope(already_ran, fut).await,
        None => fut.await,
    }
}

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

/// The layer `TypeId`s the endpoint's
/// [`McpOperationGuard`](crate::McpOperationGuard) already executed for this
/// request.
///
/// A per-operation `#[use_guards]` chain composes against this set and drops
/// what it names. Empty when nothing is installed, which is the safe default in
/// both directions: nothing is skipped for having "already run" when in fact it
/// did not.
pub fn layers_already_run() -> Arc<[TypeId]> {
    MCP_ALREADY_RAN
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::from(Vec::new()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_edge_report_is_ambient_only_inside_the_dispatch() {
        let ran: Arc<[TypeId]> = Arc::from(vec![TypeId::of::<u8>()]);

        assert!(
            layers_already_run().is_empty(),
            "no MCP dispatch is running, so nothing is installed",
        );
        let seen = with_already_run(Some(ran), async { layers_already_run() }).await;
        assert_eq!(&*seen, [TypeId::of::<u8>()]);
        assert!(
            layers_already_run().is_empty(),
            "the task-local unwinds with the operation",
        );
    }

    #[tokio::test]
    async fn a_mount_that_reports_nothing_installs_nothing() {
        let seen = with_already_run(None, async { layers_already_run() }).await;
        assert!(seen.is_empty());
    }
}
