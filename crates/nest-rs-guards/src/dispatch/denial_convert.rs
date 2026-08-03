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

/// Structural denial handling for the HTTP chain sites (route shaper,
/// self-mount fold): the one `warn` that keeps the "every denial visible at
/// warn+" invariant independent of individual guard authors, then the wire
/// conversion. Individual guards may add richer context; this line is the
/// floor.
pub(crate) fn deny_http(guard: &'static str, denial: Denial) -> Response {
    tracing::warn!(
        target: "nest_rs::layers",
        guard,
        status = denial.http_status(),
        "guard denied the request",
    );
    denial_to_http_response(denial)
}

/// Convert a transport-agnostic [`Denial`] to a poem [`Response`] on the single
/// RFC-9457 `application/problem+json` envelope — a guard denial is an
/// `Ok(4xx)` response that never travels the `Err`/`ResponseError` path, so it
/// is normalized here rather than at the transport-edge error boundary. The
/// authored 4xx reason rides as `detail`; a 5xx `Internal` keeps only the
/// generic title so no internal text leaks.
pub fn denial_to_http_response(denial: Denial) -> Response {
    let mut response = problem_response(&denial);
    // The scopes ride as a response extension rather than a header written
    // here: the challenge also has to name the metadata document, which
    // only the resource-server module knows. Attaching the evidence lets
    // that one interceptor render the whole `WWW-Authenticate` at the edge,
    // for every transport, instead of this function learning about RFC 9728.
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
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut problem = ProblemDetails::from_status(status);
    if status.is_client_error() {
        problem = problem.with_detail(denial.message().to_owned());
    }
    let mut response = problem.into_response();
    if let Denial::RateLimited {
        retry_after_secs, ..
    } = denial
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
/// **Not `Error::from_response(denial_to_http_response(d))`.** poem's
/// `Error::into_response` ends with `*resp.extensions_mut() = self.extensions`,
/// overwriting whatever the carried response held with the error's own set — so
/// that spelling silently drops the very evidence the edge needs, and does it
/// at the moment a client is being refused. `set_data` is poem's channel for
/// exactly this, and routing every denial-to-`Err` conversion through here is
/// what keeps the trap from being re-stepped-in per call site.
pub fn denial_to_http_error(denial: Denial) -> Error {
    // Built from `problem_response`, not `denial_to_http_response`: the
    // extension that one attaches is precisely what `into_response` would
    // overwrite, so putting it there would be a copy made only to be dropped.
    let mut error = Error::from_response(problem_response(&denial));
    if let Some(required) = required_scopes(&denial) {
        error.set_data(required);
    }
    error
}

/// Convert a [`Denial`] to an async-graphql error frame.
///
/// A scope denial is where GraphQL stops being the transport that learns less
/// than the others. It has no `401` to enrich — an unauthenticated operation
/// answers `200` with an `UNAUTHENTICATED` frame — but a *scope* refusal is an
/// ordinary error frame here, so the required scopes ride as a
/// `requiredScopes` extension: structurally, as a list, for the same reason
/// `forbidden_fields` does it that way.
#[cfg(feature = "graphql")]
pub fn denial_to_graphql_error(denial: Denial) -> GraphqlError {
    let code = match &denial {
        Denial::InsufficientScope { .. } => "INSUFFICIENT_SCOPE",
        _ => match denial.http_status() {
            401 => "UNAUTHENTICATED",
            403 => "FORBIDDEN",
            429 => "RATE_LIMITED",
            _ => "INTERNAL",
        },
    };
    let message = match &denial {
        Denial::Internal(_) => "internal server error".to_owned(),
        _ => denial.message().to_owned(),
    };
    let scopes = denial.required_scopes();
    let required = (!scopes.is_empty()).then(|| scopes.to_vec());
    GraphqlError::new(message).extend_with(move |_, e| {
        e.set("code", code);
        if let Some(required) = required {
            e.set("requiredScopes", required);
        }
    })
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
        // The regression this guards: `Error::from_response` alone loses them,
        // because `into_response` replaces the response's extensions wholesale.
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
}
