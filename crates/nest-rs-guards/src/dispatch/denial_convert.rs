//! Convert a transport-agnostic [`Denial`] to a transport-native error
//! shape — poem [`Response`] for HTTP, [`GraphqlError`] for GraphQL (with the
//! `graphql` feature).

use nest_rs_http::ProblemDetails;
use nest_rs_http::poem::http::{StatusCode, header};
use nest_rs_http::poem::{Error, IntoResponse, Response};

use crate::denial::Denial;
use crate::scope::RequiredScopes;

#[cfg(feature = "graphql")]
use nest_rs_graphql::async_graphql::{Error as GraphqlError, ErrorExtensions};

/// Denial handling for the HTTP chain sites (route shaper, self-mount fold): the
/// one `warn` that keeps every denial visible whatever the guard logged, then
/// the wire conversion.
pub(crate) fn deny_http(guard: &'static str, denial: Denial) -> Response {
    tracing::warn!(
        target: nest_rs_core::target::LAYERS,
        guard,
        status = denial.http_status(),
        "guard denied the request",
    );
    denial_to_http_response(denial)
}

/// Convert a transport-agnostic [`Denial`] to a poem [`Response`] on the RFC 9457
/// `application/problem+json` envelope. A 4xx reason rides as `detail`; a 5xx
/// keeps only the generic title, so no internal text leaks.
pub fn denial_to_http_response(denial: Denial) -> Response {
    let mut response = problem_response(&denial);
    // The scopes ride as an extension: the challenge also names the RFC 9728
    // metadata document, which only the oauth-resource interceptor knows.
    if let Some(required) = required_scopes(&denial) {
        response.extensions_mut().insert(required);
    }
    response
}

/// The problem+json envelope alone — everything [`denial_to_http_response`]
/// builds except the scope evidence, which the `Err` path must attach to the
/// [`Error`] instead of to this response.
fn problem_response(denial: &Denial) -> Response {
    let status = match denial.http_status() {
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        429 => StatusCode::TOO_MANY_REQUESTS,
        503 => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut problem = ProblemDetails::from_status(status);
    if status.is_client_error() {
        problem = problem.with_detail(denial.message().to_owned());
    }
    let mut response = problem.into_response();
    // RFC 6750 §3/§3.1: a rejected credential names its code. Not gated on the
    // status: `insufficient_scope` lives at `403`, and `bearer_error` is already
    // `None` wherever no challenge is due.
    if let Some(code) = denial.bearer_error() {
        nest_rs_http::challenge::stamp_bearer_error(&mut response, code);
    }
    // RFC 6585 §4 for a rate limit, RFC 9110 §15.6.4 for an unavailable
    // dependency: the wait, in delay-seconds, whenever it is known.
    if let Some(retry_after_secs) = denial.retry_after_secs()
        && let Ok(value) = retry_after_secs.to_string().parse()
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// The evidence a scope denial carries to the edge, or `None` when there is
/// none to carry — an empty set says no more than a bare `403`, so no transport
/// emits a challenge for it.
fn required_scopes(denial: &Denial) -> Option<RequiredScopes> {
    let required = denial.required_scopes();
    (!required.is_empty()).then(|| RequiredScopes::new(required.to_vec()))
}

/// Convert a [`Denial`] to a poem [`Error`] — the `Err` path's counterpart to
/// [`denial_to_http_response`], for the sites that must reject rather than
/// return (an extractor, the MCP endpoint).
///
/// Not `Error::from_response(denial_to_http_response(d))`: poem's
/// `Error::into_response` overwrites the response's extensions with the error's
/// own, dropping the scope evidence; `set_data` is the channel that survives.
pub fn denial_to_http_error(denial: Denial) -> Error {
    let mut error = Error::from_response(problem_response(&denial));
    if let Some(required) = required_scopes(&denial) {
        error.set_data(required);
    }
    error
}

/// The one sentence the in-band edges answer a denial with: a `4xx`'s own
/// reason, and a `5xx`'s condition — never what the refusal said of the server.
#[cfg(any(feature = "graphql", feature = "mcp", feature = "ws"))]
fn client_message(denial: &Denial) -> String {
    match denial {
        Denial::Internal(_) => nest_rs_core::OPAQUE_CLIENT_MESSAGE.to_owned(),
        Denial::Unavailable { .. } => nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE.to_owned(),
        _ => denial.message().to_owned(),
    }
}

/// The [`Code`](nest_rs_core::Code) a denial is known by on the edges with no
/// status line: the member a `Problem`'s class rides under there too.
#[cfg(any(feature = "graphql", feature = "mcp", feature = "ws"))]
fn denial_code(denial: &Denial) -> nest_rs_core::Code {
    use nest_rs_core::problem::code;

    match denial {
        Denial::Unauthorized(_) | Denial::InvalidCredential { .. } => code::UNAUTHENTICATED,
        Denial::Forbidden(_) => code::FORBIDDEN,
        Denial::InsufficientScope { .. } => code::INSUFFICIENT_SCOPE,
        Denial::RateLimited { .. } => code::RATE_LIMITED,
        Denial::Unavailable { .. } => code::UNAVAILABLE,
        Denial::Internal(_) => code::INTERNAL,
    }
}

/// Convert a [`Denial`] to an async-graphql error frame: its message, its
/// `extensions.code`, and the scopes or the wait it names.
///
/// The required scopes of a scope denial ride as a `requiredScopes` list
/// extension, since an error frame has no `401` to enrich. A `5xx` answers its
/// condition's sentence, never the guard's text.
#[cfg(feature = "graphql")]
pub fn denial_to_graphql_error(denial: Denial) -> GraphqlError {
    let code = denial_code(&denial).as_str();
    let scopes = denial.required_scopes();
    let required = (!scopes.is_empty()).then(|| scopes.to_vec());
    let retry_after = denial.retry_after_secs();
    GraphqlError::new(client_message(&denial)).extend_with(move |_, e| {
        e.set("code", code);
        if let Some(required) = required {
            e.set("requiredScopes", required);
        }
        if let Some(retry_after) = retry_after {
            e.set("retryAfterSeconds", retry_after);
        }
    })
}

/// Convert a [`Denial`] to the JSON-RPC error one MCP operation answers with.
///
/// MCP has no status line: the code picks the closest JSON-RPC family and the
/// `data` carries the machine-readable `code` and `reason`, plus
/// `requiredScopes` when the denial names them. An internal denial is opaque
/// (`nest_rs_mcp::Opaque`): the reader is a language model.
#[cfg(feature = "mcp")]
pub fn denial_to_mcp_error(denial: Denial) -> nest_rs_mcp::McpError {
    use nest_rs_mcp::McpError;

    if matches!(denial, Denial::Internal(_)) {
        return McpError::internal_error(nest_rs_core::OPAQUE_CLIENT_MESSAGE, None);
    }
    let data = Some(serde_json::Value::Object(structured_reason(&denial)));
    // Not the caller's request: something the server depends on did not
    // answer, which JSON-RPC files under its internal error, with the reason
    // and the wait a client acts on.
    if matches!(denial, Denial::Unavailable { .. }) {
        return McpError::internal_error(client_message(&denial), data);
    }
    McpError::invalid_request(client_message(&denial), data)
}

/// The machine-readable half of a refusal, for the two transports with no status
/// line to carry it: the `code` a client branches on, the member a `Problem`
/// answers under, and the same class as `reason`, the member these edges
/// carried first; plus `requiredScopes` when the denial names them.
#[cfg(any(feature = "mcp", feature = "ws"))]
fn structured_reason(denial: &Denial) -> serde_json::Map<String, serde_json::Value> {
    let reason = match denial {
        Denial::Unauthorized(_) | Denial::InvalidCredential { .. } => "unauthenticated",
        Denial::Forbidden(_) => "forbidden",
        Denial::InsufficientScope { .. } => "insufficient_scope",
        Denial::RateLimited { .. } => "rate_limited",
        Denial::Unavailable { .. } => "unavailable",
        Denial::Internal(_) => "internal",
    };
    let mut data = serde_json::Map::new();
    data.insert(
        "code".to_owned(),
        serde_json::Value::from(denial_code(denial).as_str()),
    );
    data.insert("reason".to_owned(), serde_json::Value::from(reason));
    let scopes = denial.required_scopes();
    if !scopes.is_empty() {
        data.insert("requiredScopes".to_owned(), serde_json::json!(scopes));
    }
    // The wait, in delay-seconds like `Retry-After`, for edges with no status line.
    if let Some(retry_after_secs) = denial.retry_after_secs() {
        data.insert(
            "retryAfterSeconds".to_owned(),
            serde_json::Value::from(retry_after_secs),
        );
    }
    data
}

/// Convert a [`Denial`] to the error frame one WS message answers with.
///
/// No status line either: the message, plus `code`, `reason` and
/// `requiredScopes` under the frame's `data.errors`, where a `Valid<T>`
/// rejection's details ride. An internal denial is opaque, and an unavailable
/// dependency answers its condition, never what the guard said of it.
#[cfg(feature = "ws")]
pub fn denial_to_ws_error(denial: Denial) -> nest_rs_ws::WsError {
    use nest_rs_ws::WsError;

    if matches!(denial, Denial::Internal(_)) {
        return WsError::new(nest_rs_core::OPAQUE_CLIENT_MESSAGE);
    }
    WsError::with_details(
        client_message(&denial),
        serde_json::Value::Object(structured_reason(&denial)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unauthorized_denial_renders_problem_json() {
        let resp = denial_to_http_response(Denial::unauthorized("missing bearer token"));
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["status"], 401);
        assert_eq!(json["title"], "Unauthorized");
        assert_eq!(json["detail"], "missing bearer token");
    }

    #[tokio::test]
    async fn rate_limited_denial_keeps_retry_after_on_problem_json() {
        let resp = denial_to_http_response(Denial::rate_limited(30, "slow down"));
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"30".as_slice()),
        );
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
    }

    #[tokio::test]
    async fn insufficient_scope_carries_the_required_scopes_to_the_edge() {
        // The transport edge is what turns these into the RFC 6750 challenge;
        // losing them here would leave a client a bare 403 it cannot act on.
        let resp = denial_to_http_response(Denial::insufficient_scope(
            ["posts:write"],
            "this token may not write posts",
        ));
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.extensions()
                .get::<RequiredScopes>()
                .map(RequiredScopes::as_slice),
            Some(["posts:write".to_owned()].as_slice()),
        );
    }

    #[tokio::test]
    async fn the_err_path_carries_the_scopes_through_poems_extension_overwrite() {
        // `Error::from_response` alone loses them: `into_response` replaces the
        // response's extensions wholesale.
        let error = denial_to_http_error(Denial::insufficient_scope(
            ["posts:write"],
            "this token may not write posts",
        ));
        let resp = error.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.extensions()
                .get::<RequiredScopes>()
                .map(RequiredScopes::as_slice),
            Some(["posts:write".to_owned()].as_slice()),
            "a denial that travels the `Err` path must reach the edge intact",
        );
    }

    #[tokio::test]
    async fn a_scope_denial_naming_nothing_is_an_ordinary_forbidden() {
        // A deployment may refuse without naming its internals; `scope=""`
        // would be a malformed challenge, so the edge must see no marker.
        let resp = denial_to_http_response(Denial::insufficient_scope(
            Vec::<String>::new(),
            "forbidden",
        ));
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(resp.extensions().get::<RequiredScopes>().is_none());
    }

    #[tokio::test]
    async fn internal_denial_is_a_500_problem_without_leaking_detail() {
        let resp = denial_to_http_response(Denial::internal("panic: secret config missing"));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            !text.contains("secret config"),
            "a 5xx denial must not leak internal detail: {text}",
        );
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(json.get("detail").is_none(), "no detail on a 500 denial");
    }

    /// All four edges report the wait, in one unit.
    #[cfg(any(feature = "mcp", feature = "ws"))]
    #[test]
    fn every_edge_reports_the_wait_a_rate_limit_denial_carries() {
        let data = structured_reason(&Denial::rate_limited(42, "too many requests"));
        assert_eq!(data["reason"], "rate_limited");
        assert_eq!(
            data["retryAfterSeconds"], 42,
            "the in-band edges carry the wait: {data:?}",
        );

        // Only a rate limit and an unavailable dependency have one — a 403
        // carrying `retryAfterSeconds` would tell a client to retry something
        // that will never succeed.
        let forbidden = structured_reason(&Denial::forbidden("nope"));
        assert!(
            forbidden.get("retryAfterSeconds").is_none(),
            "only a denial that may clear names a wait: {forbidden:?}",
        );
    }

    /// A dependency that did not answer: reason `unavailable`, its wait
    /// reported when given and never invented.
    #[cfg(any(feature = "mcp", feature = "ws"))]
    #[test]
    fn every_edge_reports_an_unavailable_dependency_and_its_known_wait() {
        let data = structured_reason(&Denial::unavailable(Some(7), "authentication unavailable"));
        assert_eq!(data["reason"], "unavailable");
        assert_eq!(data["retryAfterSeconds"], 7, "{data:?}");
        let unknown = structured_reason(&Denial::unavailable(None, "authentication unavailable"));
        assert_eq!(unknown["reason"], "unavailable");
        assert!(unknown.get("retryAfterSeconds").is_none(), "{unknown:?}");
    }

    /// The HTTP half: `503` (RFC 9110 §15.6.4), `Retry-After` when known, and
    /// the generic title of a 5xx rather than the reason.
    #[tokio::test]
    async fn an_unavailable_dependency_is_a_503_with_its_known_wait() {
        let resp =
            denial_to_http_response(Denial::unavailable(Some(7), "authentication unavailable"));
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"7".as_slice()),
        );
        let unknown =
            denial_to_http_response(Denial::unavailable(None, "authentication unavailable"));
        assert_eq!(unknown.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(unknown.headers().get(header::RETRY_AFTER).is_none());
    }

    /// What an unavailable dependency's refusal said of it — an address, a
    /// host — stays on the server on the three in-band edges, as it does on
    /// HTTP; the reason a client branches on and the wait it acts on travel.
    #[cfg(all(feature = "graphql", feature = "mcp", feature = "ws"))]
    #[test]
    fn an_unavailable_denial_tells_no_edge_what_the_dependency_said() {
        let denial = || Denial::unavailable(Some(7), "store at 10.0.0.1 down");

        let graphql = serde_json::to_value(
            denial_to_graphql_error(denial()).into_server_error(Default::default()),
        )
        .expect("serializes");
        let ws = serde_json::to_value(denial_to_ws_error(denial())).expect("serializes");
        let mcp = serde_json::to_value(denial_to_mcp_error(denial())).expect("serializes");
        for (edge, answer) in [("graphql", &graphql), ("ws", &ws), ("mcp", &mcp)] {
            assert!(
                !answer.to_string().contains("10.0.0.1"),
                "{edge} answers what the dependency said: {answer}",
            );
        }

        assert_eq!(graphql["message"], nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(graphql["extensions"]["code"], "UNAVAILABLE");
        assert_eq!(graphql["extensions"]["retryAfterSeconds"], 7);
        assert_eq!(ws["error"], nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(ws["errors"]["reason"], "unavailable");
        assert_eq!(ws["errors"]["retryAfterSeconds"], 7);
        assert_eq!(mcp["message"], nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(mcp["data"]["reason"], "unavailable");
        assert_eq!(mcp["data"]["retryAfterSeconds"], 7);
    }

    /// An internal denial reads the one constant every edge's opaque answer
    /// reads, so a client cannot tell a wiring bug from any other failure.
    #[cfg(all(feature = "graphql", feature = "mcp", feature = "ws"))]
    #[test]
    fn an_internal_denial_answers_the_opaque_message_on_every_in_band_edge() {
        let denial = || Denial::internal("redis at 10.0.3.7 refused");

        let graphql = denial_to_graphql_error(denial());
        assert_eq!(graphql.message, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        let extensions = serde_json::to_value(&graphql.extensions).expect("serializes");
        assert_eq!(extensions["code"], "INTERNAL");
        assert_eq!(
            denial_to_ws_error(denial()).error,
            nest_rs_core::OPAQUE_CLIENT_MESSAGE
        );
        assert_eq!(
            denial_to_mcp_error(denial()).message,
            nest_rs_core::OPAQUE_CLIENT_MESSAGE
        );
    }

    /// A client branches on one member, `code`, whether a guard refused it or a
    /// `Problem` answered it, on each edge with no status line.
    #[cfg(all(feature = "graphql", feature = "mcp", feature = "ws"))]
    #[test]
    fn a_denial_names_its_class_by_the_code_a_problem_carries_on_every_in_band_edge() {
        let denials = [
            (Denial::unauthorized("no token"), "UNAUTHENTICATED"),
            (
                Denial::invalid_credential("expired", "invalid_token"),
                "UNAUTHENTICATED",
            ),
            (Denial::forbidden("not yours"), "FORBIDDEN"),
            (
                Denial::insufficient_scope(["posts:write"], "too narrow"),
                "INSUFFICIENT_SCOPE",
            ),
            (Denial::rate_limited(7, "slow down"), "RATE_LIMITED"),
            (Denial::unavailable(Some(7), "store down"), "UNAVAILABLE"),
        ];
        for (denial, code) in denials {
            let graphql = serde_json::to_value(&denial_to_graphql_error(denial.clone()).extensions)
                .expect("serializes");
            let ws = serde_json::to_value(denial_to_ws_error(denial.clone())).expect("serializes");
            let mcp = serde_json::to_value(denial_to_mcp_error(denial)).expect("serializes");
            assert_eq!(graphql["code"], code, "graphql: {graphql}");
            assert_eq!(ws["errors"]["code"], code, "ws: {ws}");
            assert_eq!(mcp["data"]["code"], code, "mcp: {mcp}");
        }
    }

    /// The HTTP half of the same denial, so the two are pinned together: one
    /// number, one unit, four edges.
    #[tokio::test]
    async fn the_http_edge_reports_the_same_wait_as_a_header() {
        let resp = denial_to_http_response(Denial::rate_limited(42, "too many requests"));
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"42".as_slice()),
        );
    }
}
