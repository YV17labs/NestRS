//! The canonical name of the unit of work this edge opens.

use nest_rs_core::duration_histogram::{DurationHistogram, MetricAttribute};
use nest_rs_core::operation_log::Unit;
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
const OPERATION_DURATION: DurationHistogram = DurationHistogram::new(
    "mcp.server.operation.duration",
    "MCP request or notification duration as observed on the receiver from the time it was \
     received until the result or ack is sent.",
    &[
        0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
    ],
    &[
        MetricAttribute::known("mcp.method.name", PROTOCOL_METHODS),
        MetricAttribute::bounded(crate::propagate::RPC_RESPONSE_STATUS_CODE),
    ],
);

/// Every request method the protocol defines that this edge dispatches as a
/// unit.
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
        assert!(!is_protocol_method("tools/cal"));
        assert!(!is_protocol_method("marked/custom"));
    }
}
