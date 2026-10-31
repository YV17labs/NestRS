//! What one `/mcp` request carries to the operations rmcp runs for it, and
//! MCP's reading of `nest_rs_core::Ambient<T>`.
//!
//! rmcp dispatches every operation on its own task, where the request's
//! context is gone, but it injects the request's [`Parts`] into each
//! operation's `RequestContext`: the endpoint stashes an [`McpAmbient`] in
//! their extensions, and [`PropagatingHandler`](crate::PropagatingHandler)
//! re-installs it around the operation.
//!
//! A `#[tool]`, `#[prompt]`, `#[resource]` or `#[resources]` parameter
//! `Ambient<T>` reads what a guard attached — the `/mcp` request's guards for
//! every operation it carries, the operation's own for its body — and is no
//! argument of the protocol:
//!
//! ```
//! use nest_rs_core::{Ambient, AmbientValue, Layer, UnitContext, async_trait, injectable};
//! use nest_rs_guards::{Denial, Guard, GuardFor};
//! use nest_rs_http::{EndpointGuards, HttpView};
//! use nest_rs_mcp::{DEFAULT_PATH, McpError, McpView, mcp, tools};
//! # use nest_rs_core::module;
//! # use nest_rs_mcp::McpModule;
//! # use nest_rs_testing::mcp::{call_tool_statelessly, result};
//!
//! #[derive(Clone)]
//! struct Caller(String);
//!
//! impl AmbientValue for Caller {}
//!
//! /// Attaches the caller at the `/mcp` request, the way an authentication
//! /// guard does, and admits each operation it carries.
//! #[injectable]
//! #[derive(Layer)]
//! struct NameGuard;
//!
//! #[async_trait]
//! impl Guard for NameGuard {
//!     async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         let name = HttpView::of(cx)?.request().header("x-name").unwrap_or("anonymous");
//!         let caller = Caller(name.to_owned());
//!         cx.attach(caller);
//!         Ok(())
//!     }
//!
//!     async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         Ok(())
//!     }
//! }
//!
//! impl GuardFor<HttpView> for NameGuard {}
//!
//! #[mcp]
//! #[derive(Default)]
//! struct WhoHost;
//!
//! #[tools]
//! impl WhoHost {
//!     #[tool(description = "Say who is calling.")]
//!     #[public]
//!     async fn whoami(&self, caller: Ambient<Caller>) -> Result<String, McpError> {
//!         Ok(caller.into_inner().0)
//!     }
//! }
//! #
//! # #[module(
//! #     imports = [McpModule],
//! #     providers = [NameGuard, WhoHost, EndpointGuards<McpView, (NameGuard,)>],
//! # )]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
//! let body = call_tool_statelessly(
//!     app.http(),
//!     DEFAULT_PATH,
//!     "whoami",
//!     None,
//!     &[("x-name", "ada")],
//!     serde_json::json!({}),
//! )
//! .await;
//!
//! assert_eq!(result(&body)["result"]["content"][0]["text"], "ada");
//! # Ok(())
//! # }
//! ```

use nest_rs_core::{Ambient, AmbientValue, Failure, TaskContext};
use nest_rs_http::RequestParts as Parts;
use rmcp::model::Extensions;

use crate::error::OperationFailure;

/// What the endpoint puts in the HTTP request's extensions so it survives into
/// rmcp's per-operation `RequestContext`: the request's span and context —
/// its scope, its correlation, and what its guards and layers made ambient —
/// captured once, for a task that may outlive it.
#[derive(Clone)]
pub(crate) struct McpAmbient {
    pub(crate) carried: TaskContext,
}

impl McpAmbient {
    /// What the running request carries, captured for rmcp's tasks.
    pub(crate) fn capture() -> Self {
        Self {
            carried: TaskContext::current(),
        }
    }

    /// Read the ambient state back out of the `Parts` rmcp injects into every
    /// operation's context; `None` for an operation no endpoint request
    /// carried.
    pub(crate) fn from_extensions(extensions: &Extensions) -> Option<Self> {
        extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Self>())
            .cloned()
    }
}

/// The `Ambient<T>` an operation takes, from what its guards attached — emitted by
/// `#[tools]` in the operation's tail.
///
/// A miss is a failure the edge answers opaquely, its chain naming the type on
/// the operator's line alone.
pub fn ambient_parameter<T: AmbientValue + Clone>() -> Result<Ambient<T>, OperationFailure> {
    nest_rs_core::__private::attached::<T>()
        .map_err(|missing| OperationFailure::unanswered(Failure::from(missing)))
}
