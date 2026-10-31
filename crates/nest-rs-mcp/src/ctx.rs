//! MCP's reading of `nest_rs_core::Ctx<T>`: a `#[tool]`, `#[prompt]`,
//! `#[resource]` or `#[resources]` parameter reading what a guard attached —
//! the `/mcp` request's guards for every operation it carries, the
//! operation's own for its body — and no argument of the protocol.
//!
//! ```
//! use nest_rs_core::{AmbientValue, Ctx, Layer, UnitContext, injectable};
//! use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
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
//!     async fn whoami(&self, caller: Ctx<Caller>) -> Result<String, McpError> {
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

use nest_rs_core::{AmbientValue, Ctx, Failure};

use crate::error::OperationFailure;

/// The `Ctx<T>` an operation takes, from what its guards attached — emitted by
/// `#[tools]` in the operation's tail.
///
/// A miss is a failure the edge answers opaquely, its chain naming the type on
/// the operator's line alone.
pub fn ctx_parameter<T: AmbientValue + Clone>() -> Result<Ctx<T>, OperationFailure> {
    nest_rs_core::__private::attached::<T>()
        .map_err(|missing| OperationFailure::unanswered(Failure::from(missing)))
}
