/// A framework edge: the closed vocabulary a framework
/// [`Unit`](crate::operation_log::Unit) is declared under, the folder an edge
/// adapter lives in, and the first segment of its unit's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Edge {
    /// An HTTP route, self-mount or fallback.
    Http,
    /// A GraphQL root field or subscription.
    Graphql,
    /// A WebSocket gateway's socket and its messages.
    Ws,
    /// A queue job attempt.
    Queue,
    /// A scheduled tick.
    Schedule,
    /// An MCP operation.
    Mcp,
    /// An in-process event listener.
    Events,
}

impl Edge {
    /// Every framework edge, in the vocabulary's order.
    pub const ALL: &'static [Edge] = &[
        Self::Http,
        Self::Graphql,
        Self::Ws,
        Self::Queue,
        Self::Schedule,
        Self::Mcp,
        Self::Events,
    ];

    /// The word a unit name, a span target and a config namespace spell this
    /// edge with (`"http"`, `"graphql"`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Graphql => "graphql",
            Self::Ws => "ws",
            Self::Queue => "queue",
            Self::Schedule => "schedule",
            Self::Mcp => "mcp",
            Self::Events => "events",
        }
    }
}
