//! [`McpView`], the MCP edge's view of the operation it dispatches.

use std::borrow::Cow;

use nest_rs_core::UnitView;
use nest_rs_http::CarriedView;

use crate::operation::McpOperationKind;

/// The MCP edge's view of one operation: the JSON-RPC method, which router
/// serves it, the tool, prompt or resource it addresses, and the `#[mcp]` host
/// that declares it.
///
/// Every request is an operation a unit-stage interceptor reads through it; a
/// tool call and a prompt fetch are also the units their guards check, through
/// `UnitContext::switch_to` in `Guard::check`, and the units their exception
/// filters and handler-stage interceptors run around. It carries no
/// arguments: deciding access from a payload is a pipe's job, never a guard's.
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
#[derive(Clone, Debug)]
pub struct McpView {
    method: Cow<'static, str>,
    kind: Option<McpOperationKind>,
    name: Option<Cow<'static, str>>,
    host: Option<&'static str>,
}

impl UnitView for McpView {}

/// `/mcp`'s request carries its operations.
impl CarriedView for McpView {}

impl McpView {
    /// The view of the operation `name` on `host`, served by `kind`'s router:
    /// a tool call or a prompt fetch `#[tools]` declares.
    pub(crate) fn declared(kind: McpOperationKind, name: &'static str, host: &'static str) -> Self {
        Self {
            method: Cow::Borrowed(kind.method()),
            kind: Some(kind),
            name: Some(Cow::Borrowed(name)),
            host: Some(host),
        }
    }

    /// The view of a request for `method`, addressing `name` as the client
    /// named it, served by `kind`'s router on `host` when a host declares it.
    pub(crate) fn request(
        method: Cow<'static, str>,
        kind: Option<McpOperationKind>,
        name: Option<String>,
        host: Option<&'static str>,
    ) -> Self {
        Self {
            method,
            kind,
            name: name.map(Cow::Owned),
            host,
        }
    }

    /// The JSON-RPC method the client called (`tools/call`, `resources/read`).
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The router serving the operation: a tool call or a prompt fetch;
    /// `None` for every other request.
    pub fn kind(&self) -> Option<McpOperationKind> {
        self.kind
    }

    /// What the operation addresses: the tool or prompt it names, or a
    /// resource's URI; `None` when the method names nothing.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The `#[mcp]` host type declaring the tool or prompt the operation
    /// names; `None` when no host on the endpoint declares it, or the method
    /// names neither.
    pub fn host(&self) -> Option<&'static str> {
        self.host
    }
}
