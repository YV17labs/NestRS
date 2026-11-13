//! The canonical name of the unit of work this edge opens.

use nest_rs_core::duration_histogram::{DurationHistogram, MetricAttribute};
use nest_rs_core::operation_log::Unit;
use rmcp::model::{
    CallToolRequestMethod, CancelTaskMethod, CancelledNotificationMethod, CompleteRequestMethod,
    ConstString, DiscoverRequestMethod, GetPromptRequestMethod, GetSkillRequestMethod,
    GetTaskMethod, InitializeResultMethod, InitializedNotificationMethod, ListPromptsRequestMethod,
    ListResourceTemplatesRequestMethod, ListResourcesRequestMethod, ListSkillsRequestMethod,
    ListToolsRequestMethod, PingRequestMethod, ProgressNotificationMethod,
    ReadResourceDirectoryRequestMethod, ReadResourceRequestMethod,
    RootsListChangedNotificationMethod, SetLevelRequestMethod, SubscribeRequestMethod,
    SubscriptionsListenRequestMethod, UnsubscribeRequestMethod, UpdateTaskMethod,
};

/// One MCP operation — a request or a notification.
pub const OPERATION: Unit = nest_rs_core::unit!(
    "mcp.operation",
    target: crate::TARGET,
    kind: Server,
    duration: OPERATION_DURATION,
);

/// The MCP conventions' server duration. The span records the method as the
/// client sent it, so the duration reads it through the protocol's methods; a
/// tool or a prompt it holds as a host declares it, `_OTHER` otherwise.
const OPERATION_DURATION: DurationHistogram = DurationHistogram::new(
    "mcp.server.operation.duration",
    "MCP request or notification duration as observed on the receiver from the time it was \
     received until the result or ack is sent.",
    &[
        0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
    ],
    &[
        MetricAttribute::known(MCP_METHOD_NAME, PROTOCOL_METHODS),
        MetricAttribute::bounded(GEN_AI_TOOL_NAME),
        MetricAttribute::bounded(GEN_AI_PROMPT_NAME),
        MetricAttribute::bounded(GEN_AI_OPERATION_NAME),
        MetricAttribute::bounded(NETWORK_TRANSPORT),
        MetricAttribute::bounded(NETWORK_PROTOCOL_NAME),
        MetricAttribute::bounded(MCP_PROTOCOL_VERSION),
        MetricAttribute::bounded(RPC_RESPONSE_STATUS_CODE),
    ],
);

// The attributes OpenTelemetry's MCP conventions give an operation's span, as
// the conventions spell them; the conventions crate marks its own copies
// deprecated, the MCP conventions having left its repository.

/// The JSON-RPC method, as the client sent it.
pub(crate) const MCP_METHOD_NAME: &str = "mcp.method.name";

/// The tool a `tools/call` runs.
pub(crate) const GEN_AI_TOOL_NAME: &str = "gen_ai.tool.name";

/// The prompt a `prompts/get`, or a completion of its argument, reads.
pub(crate) const GEN_AI_PROMPT_NAME: &str = "gen_ai.prompt.name";

/// [`EXECUTE_TOOL`] on a tool call, and absent otherwise.
pub(crate) const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";

/// What [`GEN_AI_OPERATION_NAME`] says of a tool call.
pub(crate) const EXECUTE_TOOL: &str = "execute_tool";

/// The protocol version an operation runs under, as this SDK knows it.
pub(crate) const MCP_PROTOCOL_VERSION: &str = "mcp.protocol.version";

/// The URI an operation addressed, as the client sent it: the span's alone,
/// one value per resource read.
pub(crate) const MCP_RESOURCE_URI: &str = "mcp.resource.uri";

/// The JSON-RPC request's id, as the client sent it: the span's alone.
pub(crate) const JSONRPC_REQUEST_ID: &str = "jsonrpc.request.id";

/// The transport under the operation: [`TCP`], since the edge serves MCP over
/// HTTP alone.
pub(crate) const NETWORK_TRANSPORT: &str = "network.transport";

/// What [`NETWORK_TRANSPORT`] says.
pub(crate) const TCP: &str = "tcp";

/// The protocol carrying the operation: [`HTTP`].
pub(crate) const NETWORK_PROTOCOL_NAME: &str = "network.protocol.name";

/// What [`NETWORK_PROTOCOL_NAME`] says.
pub(crate) const HTTP: &str = "http";

/// A JSON-RPC error's code, as a string.
pub(crate) const RPC_RESPONSE_STATUS_CODE: &str = "rpc.response.status_code";

/// Every method the protocol defines that this edge dispatches as a unit, a
/// request's or a notification's.
const PROTOCOL_METHODS: &[&str] = &[
    CallToolRequestMethod::VALUE,
    CancelTaskMethod::VALUE,
    CancelledNotificationMethod::VALUE,
    CompleteRequestMethod::VALUE,
    DiscoverRequestMethod::VALUE,
    GetPromptRequestMethod::VALUE,
    GetSkillRequestMethod::VALUE,
    GetTaskMethod::VALUE,
    InitializeResultMethod::VALUE,
    InitializedNotificationMethod::VALUE,
    ListPromptsRequestMethod::VALUE,
    ListResourceTemplatesRequestMethod::VALUE,
    ListResourcesRequestMethod::VALUE,
    ListSkillsRequestMethod::VALUE,
    ListToolsRequestMethod::VALUE,
    PingRequestMethod::VALUE,
    ProgressNotificationMethod::VALUE,
    ReadResourceDirectoryRequestMethod::VALUE,
    ReadResourceRequestMethod::VALUE,
    RootsListChangedNotificationMethod::VALUE,
    SetLevelRequestMethod::VALUE,
    SubscribeRequestMethod::VALUE,
    SubscriptionsListenRequestMethod::VALUE,
    UnsubscribeRequestMethod::VALUE,
    UpdateTaskMethod::VALUE,
];

/// Whether `method` is one of [`PROTOCOL_METHODS`].
pub(crate) const fn is_protocol_method(method: &str) -> bool {
    let mut listed = 0;
    while listed < PROTOCOL_METHODS.len() {
        let (one, other) = (PROTOCOL_METHODS[listed].as_bytes(), method.as_bytes());
        if one.len() == other.len() {
            let mut at = 0;
            while at < one.len() && one[at] == other[at] {
                at += 1;
            }
            if at == one.len() {
                return true;
            }
        }
        listed += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(OPERATION.edge(), Some(Edge::Mcp));
    }

    #[test]
    fn a_method_the_protocol_defines_is_listed_and_a_custom_one_is_not() {
        assert!(is_protocol_method("tools/call"));
        assert!(is_protocol_method("notifications/initialized"));
        assert!(!is_protocol_method("tools/cal"));
        assert!(!is_protocol_method("marked/custom"));
    }
}
