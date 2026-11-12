//! The canonical name of the unit of work this edge opens.

use nest_rs_core::operation_log::{MetricAttribute, Unit, UnitDuration};
use rmcp::model::{
    CallToolRequestMethod, CancelTaskMethod, CompleteRequestMethod, ConstString,
    DiscoverRequestMethod, GetPromptRequestMethod, GetSkillRequestMethod, GetTaskMethod,
    InitializeResultMethod, ListPromptsRequestMethod, ListResourceTemplatesRequestMethod,
    ListResourcesRequestMethod, ListSkillsRequestMethod, ListToolsRequestMethod, PingRequestMethod,
    ReadResourceDirectoryRequestMethod, ReadResourceRequestMethod, SetLevelRequestMethod,
    SubscribeRequestMethod, SubscriptionsListenRequestMethod, UnsubscribeRequestMethod,
    UpdateTaskMethod,
};

/// One MCP operation — a request or a notification.
pub const OPERATION: Unit = nest_rs_core::unit!(
    "mcp.operation",
    target: crate::TARGET,
    kind: Server,
    duration: OPERATION_DURATION,
);

/// The MCP conventions' server duration. The span records the method as the
/// client sent it, so the duration reads it through the protocol's methods.
const OPERATION_DURATION: UnitDuration = UnitDuration::new(
    "mcp.server.operation.duration",
    "MCP request or notification duration as observed on the receiver from the time it was \
     received until the result or ack is sent.",
    &[
        MetricAttribute::known("mcp.method.name", PROTOCOL_METHODS),
        MetricAttribute::bounded("rpc.response.status_code"),
        MetricAttribute::bounded("error.type"),
    ],
);

/// Every request method the protocol defines that this edge dispatches as a
/// unit; one missing reads `_OTHER` on the duration.
const PROTOCOL_METHODS: &[&str] = &[
    CallToolRequestMethod::VALUE,
    CancelTaskMethod::VALUE,
    CompleteRequestMethod::VALUE,
    DiscoverRequestMethod::VALUE,
    GetPromptRequestMethod::VALUE,
    GetSkillRequestMethod::VALUE,
    GetTaskMethod::VALUE,
    InitializeResultMethod::VALUE,
    ListPromptsRequestMethod::VALUE,
    ListResourceTemplatesRequestMethod::VALUE,
    ListResourcesRequestMethod::VALUE,
    ListSkillsRequestMethod::VALUE,
    ListToolsRequestMethod::VALUE,
    PingRequestMethod::VALUE,
    ReadResourceDirectoryRequestMethod::VALUE,
    ReadResourceRequestMethod::VALUE,
    SetLevelRequestMethod::VALUE,
    SubscribeRequestMethod::VALUE,
    SubscriptionsListenRequestMethod::VALUE,
    UnsubscribeRequestMethod::VALUE,
    UpdateTaskMethod::VALUE,
];

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(OPERATION.edge(), Some(Edge::Mcp));
    }
}
