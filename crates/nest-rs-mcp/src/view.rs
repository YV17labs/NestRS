//! [`McpView`], the MCP edge's view of the operation it dispatches.

use nest_rs_core::UnitView;
use nest_rs_http::CarriedView;

use crate::operation::McpOperationKind;

/// The MCP edge's view of one operation a guard checks: the JSON-RPC method,
/// which router serves it, the tool or prompt it names, and its `#[mcp]` host.
///
/// A guard reaches it through `UnitContext::switch_to` in `Guard::check`. It
/// carries no arguments: deciding access from a payload is a pipe's job, never
/// a guard's.
///
/// It also names `/mcp` to [`EndpointGuards`](nest_rs_http::EndpointGuards):
/// the guards a module declares for `McpView` run at every `/mcp` request.
///
/// ```
/// use nest_rs_core::{Layer, UnitContext, injectable, module};
/// use nest_rs_guards::{Denial, Guard, GuardFor};
/// use nest_rs_http::{EndpointGuards, HttpView, StatusCode, async_trait};
/// use nest_rs_mcp::McpView;
/// # use nest_rs_mcp::{McpError, mcp, tools};
/// # use nest_rs_testing::TestApp;
/// # use nest_rs_testing::mcp::{initialize_request, post_message_with};
///
/// #[injectable]
/// #[derive(Default)]
/// struct ApiKeyGuard;
///
/// impl Layer for ApiKeyGuard {}
///
/// #[async_trait]
/// impl Guard for ApiKeyGuard {
///     async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
///         match HttpView::of(cx)?.request().header("x-api-key") {
///             Some(_) => Ok(()),
///             None => Err(Denial::unauthorized("an API key is required")),
///         }
///     }
/// }
///
/// impl GuardFor<HttpView> for ApiKeyGuard {}
/// #
/// # #[mcp]
/// # #[derive(Default)]
/// # struct ReportsHost;
/// #
/// # #[tools]
/// # impl ReportsHost {
/// #     #[tool(description = "Count the reports.")]
/// #     #[public]
/// #     async fn count(&self) -> Result<String, McpError> {
/// #         Ok("0".into())
/// #     }
/// # }
///
/// #[module(providers = [ApiKeyGuard, ReportsHost, EndpointGuards<McpView, (ApiKeyGuard,)>])]
/// struct ReportsModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = TestApp::for_module::<ReportsModule>().await?;
/// # let init = initialize_request();
///
/// let anonymous = post_message_with(app.http(), "/mcp", None, None, &[], &init).await;
/// anonymous.assert_status(StatusCode::UNAUTHORIZED);
///
/// let keyed = [("x-api-key", "k")];
/// post_message_with(app.http(), "/mcp", None, None, &keyed, &init).await.assert_status_is_ok();
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct McpView {
    kind: McpOperationKind,
    name: &'static str,
    host: &'static str,
}

impl UnitView for McpView {}

/// `/mcp`'s request carries its operations.
impl CarriedView for McpView {}

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
