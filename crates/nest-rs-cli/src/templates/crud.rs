//! The handler variables a transport adapter renders over a CRUD port.

use crate::naming::Transport;

/// The body a skeleton renders when it must name the injected service without
/// calling it — the generated project builds under its own `clippy -D warnings`,
/// so an unread `svc` is a compile failure the developer inherits.
///
/// One constant because both branches below reach for it: a re-indent of the
/// generated body has to stay one edit.
const KEEP_SVC_LIVE: &str = "        let _ = &self.svc;";

/// The handler a transport adapter renders, as `{{op}}` / `{{op_body}}` /
/// `{{op_value}}` / `{{op_description}}`.
///
/// One template per transport, two bodies. Over a `g feature` port the handler
/// delegates to the service's `count()`, showing the seam. Over a `g resource`
/// port it cannot — a `CrudService` has no `count()`, and its rows are only
/// reachable behind an ambient ability a skeleton must not fabricate — so the
/// body is a placeholder naming the call to write. Only the handler differs
/// between the two, so only the handler is a variable: the imports, the
/// `#[gateway]`/`#[tool_router]` scaffolding and the `/ws/<feature>` path
/// rationale keep one home each.
pub fn crud_vars(crud_port: bool, transport: Transport) -> Vec<(&'static str, String)> {
    if !crud_port {
        return vec![
            ("op", "count".to_owned()),
            // Only the schedule tick renders no `{{op_value}}`, so it is the one
            // transport whose injected `svc` would go unread — the same
            // placeholder the resource-port bodies use keeps it live until the
            // developer writes the tick.
            (
                "op_body",
                match transport {
                    Transport::Schedule => KEEP_SVC_LIVE,
                    _ => "",
                }
                .to_owned(),
            ),
            // An MCP tool answers with an owned `String`, so it renders the
            // conversion rather than a borrow the generated project's own
            // `clippy -D warnings` would reject.
            (
                "op_value",
                match transport {
                    Transport::Mcp => "self.svc.count().to_string()",
                    _ => "&self.svc.count()",
                }
                .to_owned(),
            ),
            ("op_description", "Count {{kebab}} items.".to_owned()),
        ];
    }

    let body = match transport {
        // A skeleton over a resource port names the service and returns a
        // placeholder: its rows are ability-scoped, and an ambient `Ability` is
        // what the adapter's authz module installs — none of which a scaffold
        // may fabricate. The generated `AGENTS.md` carries the posture each edge
        // then declares; the placeholder is what keeps `svc` used until it does.
        Transport::Ws | Transport::Schedule | Transport::Mcp => KEEP_SVC_LIVE,
        // These three render no `{{op_body}}` over a resource: HTTP and GraphQL
        // take a template of their own (`resource::HTTP_CONTROLLER` /
        // `GRAPHQL_RESOLVER_CRUD`), and the queue processor is driven by its
        // payload type rather than by a read. Spelled out rather than left to
        // `_` so a transport added later has to choose a body on purpose.
        Transport::Http | Transport::Graphql | Transport::Queue => "",
    };
    let value = match transport {
        Transport::Ws => "&Vec::<String>::new()",
        Transport::Mcp => "\"[]\".to_owned()",
        _ => "\"[]\".to_string()",
    };
    vec![
        ("op", "list".to_owned()),
        ("op_body", body.to_owned()),
        ("op_value", value.to_owned()),
        ("op_description", "List {{kebab}} items.".to_owned()),
    ]
}
