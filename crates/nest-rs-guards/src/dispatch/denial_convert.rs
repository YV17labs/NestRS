//! Convert a transport-agnostic [`Denial`] to a transport-native error
//! shape — poem [`Response`] for HTTP, [`GraphqlError`] for GraphQL (with the
//! `graphql` feature).

use nest_rs_http::ProblemDetails;
use nest_rs_http::poem::http::{StatusCode, header};
use nest_rs_http::poem::{IntoResponse, Response};

use crate::denial::Denial;

#[cfg(feature = "graphql")]
use nest_rs_graphql::async_graphql::{Error as GraphqlError, ErrorExtensions};

/// Convert a transport-agnostic [`Denial`] to a poem [`Response`] on the single
/// RFC-9457 `application/problem+json` envelope — a guard denial is an
/// `Ok(4xx)` response that never travels the `Err`/`ResponseError` path, so it
/// is normalized here rather than at the transport-edge error boundary. The
/// authored 4xx reason rides as `detail`; a 5xx `Internal` keeps only the
/// generic title so no internal text leaks.
pub fn denial_to_http_response(denial: Denial) -> Response {
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
    } = &denial
        && let Ok(value) = retry_after_secs.to_string().parse()
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Convert a [`Denial`] to an async-graphql error frame.
#[cfg(feature = "graphql")]
pub fn denial_to_graphql_error(denial: Denial) -> GraphqlError {
    let code = match denial.http_status() {
        401 => "UNAUTHENTICATED",
        403 => "FORBIDDEN",
        429 => "RATE_LIMITED",
        _ => "INTERNAL",
    };
    let message = match &denial {
        Denial::Internal(_) => "internal server error".to_owned(),
        _ => denial.message().to_owned(),
    };
    GraphqlError::new(message).extend_with(|_, e| e.set("code", code))
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
