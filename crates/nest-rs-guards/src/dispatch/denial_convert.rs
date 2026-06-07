//! Convert a transport-agnostic [`Denial`] to a transport-native error
//! shape — poem [`Response`] for HTTP, [`GraphqlError`] for GraphQL.

use nest_rs_graphql::async_graphql::{Error as GraphqlError, ErrorExtensions};
use nest_rs_http::poem::http::StatusCode;
use nest_rs_http::poem::{Body, Response};

use crate::denial::Denial;

/// Convert a transport-agnostic [`Denial`] to a poem [`Response`].
pub fn denial_to_http_response(denial: Denial) -> Response {
    let status = match denial.http_status() {
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        429 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut builder = Response::builder().status(status);
    if let Denial::RateLimited {
        retry_after_secs, ..
    } = &denial
    {
        builder = builder.header("Retry-After", retry_after_secs.to_string());
    }
    builder.body(Body::from_string(denial.message().to_owned()))
}

/// Convert a [`Denial`] to an async-graphql error frame.
pub fn denial_to_graphql_error(denial: Denial) -> GraphqlError {
    let code = match denial.http_status() {
        401 => "UNAUTHENTICATED",
        403 => "FORBIDDEN",
        429 => "RATE_LIMITED",
        _ => "INTERNAL",
    };
    GraphqlError::new(denial.message().to_owned()).extend_with(|_, e| e.set("code", code))
}
