//! [`McpView`], the MCP edge's view of the operation it dispatches.

use std::borrow::Cow;

use nest_rs_core::{ServedAddress, UnitView};
use nest_rs_http::CarriedView;

use crate::operation::McpOperationKind;

/// The MCP edge's view of one operation: the JSON-RPC method, which router
/// serves it, the tool, prompt or resource it addresses, and the `#[mcp]` host
/// that declares it.
///
/// Every request is an operation a unit-stage interceptor reads through it; an
/// operation `#[tools]` declares — a tool call, a prompt fetch, a resource
/// read or listing — is also the unit its guards check, through
/// `UnitContext::switch_to` in `Guard::check`, and the units their exception
/// filters and handler-stage interceptors run around. It carries no
/// arguments: deciding access from a payload is a pipe's job, never a guard's.
///
/// It also names `/mcp` to [`EndpointGuards`](nest_rs_http::EndpointGuards):
/// the guards a module declares for `McpView` run at every `/mcp` request.
/// An API key names no subject, so the calls it admits are stateless: a
/// session is bound to the subject an authentication guard names.
///
/// ```
/// use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
/// use nest_rs_guards::{Denial, Guard, GuardFor};
/// use nest_rs_http::{EndpointGuards, HttpView};
/// use nest_rs_mcp::McpView;
/// # use nest_rs_mcp::{McpError, mcp, tools};
/// # use nest_rs_testing::TestApp;
/// # use nest_rs_testing::mcp::{call_tool_statelessly, result};
/// # use serde_json::{Value, json};
///
/// #[injectable]
/// #[derive(Layer)]
/// struct ApiKeyGuard;
///
/// #[async_trait]
/// impl Guard for ApiKeyGuard {
///     async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
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
///
/// let keyed = [("x-api-key", "k")];
/// let refused = call_tool_statelessly(app.http(), "/mcp", "count", None, &[], json!({})).await;
/// let counted = call_tool_statelessly(app.http(), "/mcp", "count", None, &keyed, json!({})).await;
///
/// assert_eq!(serde_json::from_str::<Value>(&refused)?["status"], 401);
/// assert_eq!(result(&counted)["result"]["content"][0]["text"], "0");
/// # Ok(())
/// # }
/// ```
///
/// It is read in place, never copied: it stays where a layer's answer can be
/// written, as every edge's view is, and a layer keeps what it reads of it by
/// value.
///
/// ```compile_fail,E0308
/// use nest_rs_mcp::McpView;
///
/// fn kept(view: &McpView) -> McpView {
///     view.clone()
/// }
/// ```
#[derive(Debug)]
pub struct McpView {
    method: Cow<'static, str>,
    kind: Option<McpOperationKind>,
    name: Option<Cow<'static, str>>,
    host: Option<&'static str>,
}

impl UnitView for McpView {
    fn served_address(&self) -> Option<ServedAddress<'_>> {
        // An owned name is the one the client wrote, which reaches no line.
        let Cow::Borrowed(name) = self.name.as_ref()? else {
            return None;
        };
        Some(match self.kind? {
            McpOperationKind::Tool => ServedAddress::tool(name),
            McpOperationKind::Prompt => ServedAddress::prompt(name),
            McpOperationKind::Resource | McpOperationKind::Resources => {
                ServedAddress::resource(name)
            }
        })
    }
}

/// `/mcp`'s request carries its operations.
impl CarriedView for McpView {}

impl McpView {
    /// The view of the operation `name` on `host`, served by `kind`'s router:
    /// an operation `#[tools]` declares.
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

    /// The router serving the operation: a tool call, a prompt fetch, a
    /// resource read, or — inside its site — a resource listing; `None` for
    /// every other request.
    pub fn kind(&self) -> Option<McpOperationKind> {
        self.kind
    }

    /// What the operation addresses: the tool or prompt it names, a
    /// resource's URI as the request named it — inside a `#[resource]`'s site,
    /// the template it reads through, never the values the URI carries — or a
    /// `#[resources]` listing's method; `None` when the method names nothing.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The `#[mcp]` host type declaring the operation; `None` when no host on
    /// the endpoint declares it, or — for a request, before a host matched a
    /// resource's URI to a template — when only the host can say.
    pub fn host(&self) -> Option<&'static str> {
        self.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_operation_states_where_it_is_served() {
        for (kind, name, served) in [
            (McpOperationKind::Tool, "ping", ServedAddress::tool("ping")),
            (
                McpOperationKind::Prompt,
                "greet",
                ServedAddress::prompt("greet"),
            ),
            (
                McpOperationKind::Resource,
                "memo://{id}",
                ServedAddress::resource("memo://{id}"),
            ),
        ] {
            let view = McpView::declared(kind, name, "Host");
            assert_eq!(view.served_address(), Some(served));
        }
    }

    /// What a client wrote reaches no line: the name a request carries before
    /// its site says the server's own.
    #[test]
    fn a_name_the_client_wrote_is_stated_nowhere() {
        let view = McpView::request(
            Cow::Borrowed("tools/call"),
            Some(McpOperationKind::Tool),
            Some("a client's name".to_owned()),
            None,
        );
        assert_eq!(view.served_address(), None);
    }
}
