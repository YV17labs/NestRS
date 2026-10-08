//! The handler variables a transport adapter renders over a CRUD port.

use crate::naming::Transport;

/// The body a skeleton renders when it must name the injected service without
/// calling it: the generated project's `clippy -D warnings` rejects an unread
/// `svc`.
const KEEP_SVC_LIVE: &str = "        let _ = &self.svc;";

/// The handler a transport adapter renders, as `{{op}}` / `{{op_body}}` /
/// `{{op_value}}` / `{{op_description}}`.
///
/// Over a `g feature` port the handler delegates to the service's `count()`;
/// over a `g resource` port the body is a placeholder naming the call to write,
/// since a `CrudService` has no `count()` and its rows need an ambient ability
/// a skeleton must not fabricate.
pub(crate) fn crud_vars(crud_port: bool, transport: Transport) -> Vec<(&'static str, String)> {
    if !crud_port {
        return vec![
            ("op", "count".to_owned()),
            // The schedule tick renders no `{{op_value}}`, so the placeholder keeps its
            // `svc` read.
            (
                "op_body",
                match transport {
                    Transport::Schedule => KEEP_SVC_LIVE,
                    _ => "",
                }
                .to_owned(),
            ),
            // An MCP tool answers with an owned `String`.
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
        Transport::Ws | Transport::Schedule | Transport::Mcp => KEEP_SVC_LIVE,
        // HTTP and GraphQL take a template of their own over a resource, and the
        // queue processor is driven by its payload; no `_`, so a new transport chooses.
        Transport::Http | Transport::Graphql | Transport::Queue | Transport::Events => "",
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
