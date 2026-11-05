//! [`ServedAddress`], what a line names a unit by, beside its host and method.

/// Where a unit is served, in the fields its edge's `mounted` line names it by:
/// what joins a line about one unit, a guard's denial, to the line that mounted
/// it. It holds the server's values only, never what a client addressed.
///
/// An edge's [`UnitView`](crate::UnitView) states it; on a guard's denial, a view
/// stating none has its [`Handler::address`](crate::Handler::address) said
/// instead.
///
/// ```
/// use nest_rs_core::ServedAddress;
///
/// let route = ServedAddress::route("GET", Some("/posts/{id}"));
/// assert_eq!(route.verb, Some("GET"));
/// assert_eq!(route.path, Some("/posts/{id}"));
/// assert_eq!(route.event, None);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServedAddress<'a> {
    /// The HTTP method of a request, as the conventions or a route spell it.
    pub verb: Option<&'a str>,
    /// The route template a request matched, under the app's prefix; on a
    /// self-mount the one it matched, `/files/{*name}` where its mount's line says
    /// `/files`.
    pub path: Option<&'a str>,
    /// The kind of operation a GraphQL field runs under: `query`, `mutation`
    /// or `subscription`.
    pub kind: Option<&'a str>,
    /// A GraphQL field, as its site declares it.
    pub field: Option<&'a str>,
    /// A WebSocket event, as its `#[subscribe_message]` declares it.
    pub event: Option<&'a str>,
    /// An MCP tool, by the name it declares.
    pub tool: Option<&'a str>,
    /// An MCP prompt, by the name it declares.
    pub prompt: Option<&'a str>,
    /// An MCP resource, by the template it declares.
    pub resource: Option<&'a str>,
}

impl<'a> ServedAddress<'a> {
    /// An HTTP request: its `verb`, and the template it matched, `None` where
    /// no route matched.
    pub const fn route(verb: &'a str, path: Option<&'a str>) -> Self {
        Self {
            verb: Some(verb),
            path,
            ..Self::NOWHERE
        }
    }

    /// A GraphQL `field`, run under an operation of `kind`.
    pub const fn field(kind: &'a str, field: &'a str) -> Self {
        Self {
            kind: Some(kind),
            field: Some(field),
            ..Self::NOWHERE
        }
    }

    /// A WebSocket message's `event`.
    pub const fn event(event: &'a str) -> Self {
        Self {
            event: Some(event),
            ..Self::NOWHERE
        }
    }

    /// An MCP `tool`.
    pub const fn tool(tool: &'a str) -> Self {
        Self {
            tool: Some(tool),
            ..Self::NOWHERE
        }
    }

    /// An MCP `prompt`.
    pub const fn prompt(prompt: &'a str) -> Self {
        Self {
            prompt: Some(prompt),
            ..Self::NOWHERE
        }
    }

    /// An MCP `resource`, by its template.
    pub const fn resource(resource: &'a str) -> Self {
        Self {
            resource: Some(resource),
            ..Self::NOWHERE
        }
    }

    /// No coordinate at all, what the others start from.
    const NOWHERE: Self = Self {
        verb: None,
        path: None,
        kind: None,
        field: None,
        event: None,
        tool: None,
        prompt: None,
        resource: None,
    };
}
